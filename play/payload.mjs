// Concern: spells one example, its target and node texts, as a URL fragment and back | Non-concern: which examples the README holds (links.mjs), rendering one (play.js) | IO: (example) <-> fragment text

async function piped(bytes, stream) {
  const out = new Blob([bytes]).stream().pipeThrough(stream);
  return new Uint8Array(await new Response(out).arrayBuffer());
}

/** JSON, deflate-raw, base64url. */
export async function encode(example) {
  const json = new TextEncoder().encode(JSON.stringify(example));
  const packed = await piped(json, new CompressionStream("deflate-raw"));
  let text = "";
  for (const byte of packed) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export async function decode(fragment) {
  const text = atob(fragment.replace(/-/g, "+").replace(/_/g, "/"));
  const bytes = Uint8Array.from(text, (c) => c.charCodeAt(0));
  const json = await piped(bytes, new DecompressionStream("deflate-raw"));
  const example = JSON.parse(new TextDecoder().decode(json));
  const nodes = example?.nodes;
  const texts = nodes && typeof nodes === "object" ? Object.values(nodes) : [null];
  if (typeof example?.target !== "string" || texts.some((t) => typeof t !== "string")) {
    throw new TypeError(
      `the fragment decodes to ${JSON.stringify(example)}, not { target: string, nodes: { name: text } }`,
    );
  }
  return example;
}
