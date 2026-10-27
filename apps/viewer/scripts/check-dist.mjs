// Checks the viewer's build against the policy it is served under (M13 slice 4, ADR-0051): `default-src 'self';
// script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; img-src 'self' blob:`. It is the last step of `pnpm build` and
// the test `src/dist.test.ts` runs it too. Problems are returned as sentences; none is a good build.
//
// What it looks for:
//   - in `index.html`: no inline script, no inline style, no event-handler or `style` attribute, every address relative to
//     `/view/`, the policy meta tag, the referrer meta tag;
//   - in the scripts and styles: no `data:` address that a style or a picture would load (the policy has no `data:`
//     source), and no remote address that is not on the list of strings known to be inert (the names of XML namespaces,
//     the addresses in error messages and comments of the libraries): a new host is either a request the policy would
//     refuse or a string to look at, and the list says which was decided.

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/**
 * Hosts that appear in the bundle as text and are not requested by the viewer's own code: XML namespaces and documentation
 * links in the libraries' messages and comments, and the addresses inside the vendored libraries' features that the policy
 * refuses whatever they do (Excalidraw's collaboration, library and font servers and its embed allow-list, mermaid's
 * documentation, the Place editor's map link, pdf.js' sample address). The page is bundled with the whole editor, so these
 * strings come with it; a request to any of them fails under `connect-src 'self'` and `img-src 'self' blob:`. A host that is
 * not here is new: either a request the policy would refuse (look at the code that makes it) or one more such string (add it).
 */
export const INERT_HOSTS = new Set([
  // namespaces, documentation and error-message links
  "www.w3.org",
  "w3.org",
  "react.dev",
  "reactjs.org",
  "rolldown.rs",
  "github.com",
  "gist.github.com",
  "developer.mozilla.org",
  "en.wikipedia.org",
  "localhost",
  "example.com",
  "sync.example.com",
  "foo.bar",
  "www.eclipse.org",
  "prosemirror.net",
  "mathml.js.org",
  "katex.org",
  "mermaid.js.org",
  "chevrotain.io",
  "langium.org",
  // the Place editor's "open in a map" link
  "www.openstreetmap.org",
  // Excalidraw: its servers, and the sites its embeddable-link allow-list names
  "excalidraw.com",
  "docs.excalidraw.com",
  "json.excalidraw.com",
  "libraries.excalidraw.com",
  "plus.excalidraw.com",
  "app.excalidraw.com",
  "oss-ai.excalidraw.com",
  "oss-collab.excalidraw.com",
  "us-central1-excalidraw-room-persistence.cloudfunctions.net",
  "excalidraw-room-persistence.firebaseio.com",
  "esm.sh",
  "www.youtube.com",
  "youtube.com",
  "player.vimeo.com",
  "www.figma.com",
  "twitter.com",
  "platform.twitter.com",
  "x.com",
  "reddit.com",
  "embed.reddit.com",
  "giphy.com",
  "discord.gg",
]);

/** @param {string} dir the build output directory (`apps/viewer/dist`) @returns {string[]} */
export function checkDist(dir) {
  /** @type {string[]} */
  const problems = [];
  const indexPath = join(dir, "index.html");
  if (!existsSync(indexPath)) {
    return [`${indexPath} does not exist: nothing was built`];
  }
  const html = readFileSync(indexPath, "utf8");

  if (/<script(?![^>]*\bsrc=)[^>]*>/iu.test(html)) problems.push("index.html has an inline script");
  if (/<style/iu.test(html)) problems.push("index.html has a style element");
  if (/\son[a-z]+=/iu.test(html)) problems.push("index.html has an event-handler attribute");
  if (/\sstyle=/iu.test(html)) problems.push("index.html has a style attribute");
  if (!/http-equiv="Content-Security-Policy"/iu.test(html)) problems.push("index.html lacks the policy meta tag");
  if (!/<meta name="referrer" content="no-referrer"/iu.test(html)) problems.push("index.html lacks the referrer meta tag");
  const addresses = [...html.matchAll(/\s(?:src|href)="([^"]*)"/giu)].map((match) => match[1] ?? "");
  for (const address of addresses) {
    if (!address.startsWith("/view/")) problems.push(`index.html names an address that is not under /view/: ${address}`);
    else if (!existsSync(join(dir, address.slice("/view/".length)))) problems.push(`index.html names a file the build did not make: ${address}`);
  }
  if (!addresses.some((address) => address.endsWith(".js"))) problems.push("index.html loads no script");

  const assets = join(dir, "assets");
  const files = existsSync(assets) ? readdirSync(assets) : [];
  if (!files.some((file) => /^kb_wasm_bg-.*\.wasm$/u.test(file))) problems.push("assets/ has no kb_wasm_bg-<hash>.wasm");
  for (const file of files) {
    // Assets are cached as immutable: every file name carries its hash.
    if (!/-[A-Za-z0-9_-]{6,}\.[a-z0-9]+$/u.test(file)) problems.push(`assets/${file} has no hash in its name`);
    if (!/\.(?:js|css)$/u.test(file)) continue;
    const text = readFileSync(join(assets, file), "utf8");
    if (file.endsWith(".css")) {
      for (const match of text.matchAll(/url\(\s*["']?(data:[^)"']{0,40})/giu)) {
        // Excalidraw's own stylesheet (a whiteboard's editor, loaded when one is drawn) has swatch pictures as data: URLs
        // that the policy refuses; a whiteboard in a shared page shows without them. Any other stylesheet must have none.
        const rule = text.slice(text.lastIndexOf("}", match.index) + 1, match.index).trimStart();
        if (!rule.startsWith(".excalidraw")) {
          problems.push(`assets/${file} loads a data: address (${match[1] ?? ""}...), which the policy refuses`);
        }
      }
    }
    const hosts = new Set();
    for (const match of text.matchAll(/https?:\/\/([a-z0-9.-]+\.[a-z]{2,}|localhost)/giu)) {
      hosts.add((match[1] ?? "").toLowerCase());
    }
    for (const host of hosts) {
      if (!INERT_HOSTS.has(host)) problems.push(`assets/${file} names the remote host ${host}`);
    }
  }
  return problems;
}

const here = dirname(fileURLToPath(import.meta.url));
if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const problems = checkDist(join(here, "..", "dist"));
  if (problems.length > 0) {
    console.error(`check-dist: the build does not fit its policy:\n- ${problems.join("\n- ")}`);
    process.exit(1);
  }
  console.log("check-dist: the build fits its policy (no inline code, no remote address, no data: address)");
}
