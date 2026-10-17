#!/usr/bin/env node
// Concern: writes each README [Play] link from the example it plays, or checks each is current | Non-concern: the fragment's spelling (payload.mjs), rendering (play.js) | IO: (README.md, --check) -> README.md, or exit 1

// An `@` title is the target; another, an expression played as `equation`; a link just above a `;` fenced file names it.

import { readFileSync, writeFileSync } from "node:fs";
import { decode, encode } from "./payload.mjs";

const PAGE = "https://fredrikryden.github.io/scalable-vector-audio/";
const README = new URL("../README.md", import.meta.url);
const LINK = /\[Play\]\(([^ )]*) "([^"]*)"\)/g;
const REF = /@([A-Za-z_][\w/]*)/g;
const EQUATION = "equation";

const check = process.argv.includes("--check");
const text = readFileSync(README, "utf8");
const lines = text.split("\n");
const problems = [];

const fenced = new Set();
const blocks = [];
for (let i = 0, open = null; i < lines.length; i++) {
  if (lines[i].startsWith("```")) {
    if (open === null) open = i;
    else {
      blocks.push({ at: open, body: lines.slice(open + 1, i).join("\n") + "\n", bound: false });
      open = null;
    }
  } else if (open !== null) fenced.add(i);
}

const links = [];
lines.forEach((line, i) => {
  if (fenced.has(i)) return;
  for (const m of line.matchAll(LINK)) links.push({ line: i, url: m[1], title: m[2] });
  if (line.startsWith("$`") && !line.includes("[Play](")) {
    problems.push(`README.md:${i + 1}: an equation with no [Play] link`);
  }
});

const nodes = new Map();
for (const link of links) {
  let next = link.line;
  while (next < lines.length && lines[next].trim() !== "") next++;
  while (next < lines.length && lines[next].trim() === "") next++;
  const block = blocks.find((b) => b.at === next && b.body.startsWith(";"));
  if (!block) continue;
  const name = /^@([A-Za-z_][\w/]*)/.exec(link.title)?.[1];
  if (!name) {
    problems.push(`README.md:${link.line + 1}: "${link.title}" names no node for the file below it`);
    continue;
  }
  if (nodes.has(name) && nodes.get(name) !== block.body) {
    problems.push(`README.md:${link.line + 1}: \`${name}\` names two different files`);
  }
  nodes.set(name, block.body);
  block.bound = true;
}
for (const block of blocks.filter((b) => b.body.startsWith(";") && !b.bound)) {
  problems.push(`README.md:${block.at + 1}: a node file with no [Play] link in the paragraph before it`);
}

function example(link) {
  const own = link.title.startsWith("@") ? {} : { [EQUATION]: `${link.title}\n` };
  const target = link.title.startsWith("@") ? link.title : `@${EQUATION}([0, 2s])`;
  const held = { ...own };
  const unread = [target, ...Object.values(own)];
  while (unread.length > 0) {
    for (const [, name] of unread.pop().matchAll(REF)) {
      if (name in held) continue;
      if (!nodes.has(name)) {
        problems.push(`README.md:${link.line + 1}: "${link.title}" reads \`${name}\`, no file in the README`);
        continue;
      }
      held[name] = nodes.get(name);
      unread.push(held[name]);
    }
  }
  const sorted = Object.fromEntries(Object.entries(held).sort(([a], [b]) => (a < b ? -1 : 1)));
  return { target, nodes: sorted };
}

let written = text;
for (const link of links) {
  const want = example(link);
  if (check) {
    const fragment = link.url.startsWith(`${PAGE}#`) ? link.url.slice(PAGE.length + 1) : null;
    const held = fragment === null ? null : await decode(fragment).catch(() => null);
    if (JSON.stringify(held) !== JSON.stringify(want)) {
      problems.push(`README.md:${link.line + 1}: the [Play] link for "${link.title}" is stale`);
    }
  } else {
    const url = `${PAGE}#${await encode(want)}`;
    written = written.replace(`[Play](${link.url} "${link.title}")`, `[Play](${url} "${link.title}")`);
  }
}

if (problems.length > 0) {
  console.error(problems.join("\n"));
  console.error(check ? "run `node play/links.mjs` to rewrite the links" : "README.md left unchanged");
  process.exit(1);
}
if (!check) writeFileSync(README, written);
console.log(`${links.length} [Play] links ${check ? "current" : "written"}`);
