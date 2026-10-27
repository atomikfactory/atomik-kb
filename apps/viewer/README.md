# @atomik-kb/viewer

The web viewer of view-only links (see "View links" in the kb-server README). A link is
`https://<server>/view#<key>`: the page opens read-only in the browser of someone with no account, and the server that
serves it never sees the page or the key.

## How it works

1. `src/main.tsx` reads the key from `location.hash` and removes it from the address bar at once
   (`history.replaceState`). The key lives in the script's memory from then on, so **a reload needs the full link again**
   (the page says so: "This link has no key"). The browser never sends a fragment to the server, and nothing in the viewer
   puts the key in a request: it is only the input of `kb-wasm`.
2. `kb-wasm` (`crates/kb-wasm`, built to `src/wasm`) derives the link id from the key. `ViewerTransport` fetches
   `GET /v2/links/{id}` (same origin, `credentials: "omit"`, `referrerPolicy: "no-referrer"`, `cache: "no-store"`) and
   `kb-wasm` opens the publication under the key. A page picture is `GET /v2/links/{id}/chunks/{hash}`, decrypted by
   `kb-wasm` (the attachment keys never leave the module).
3. The page is drawn by `@atomik-kb/ui`'s `ReadOnlyPageView` (`packages/ui/src/viewer/`): the app's page editor, locked, with the
   cover, icon and title, over a `CoreClient` whose transport is `ViewerTransport`. The page's document is the publication's
   shallow snapshot, imported with `loro-crdt`. Mentions show the names the publication carries.
4. Every 60 seconds (`LINK_POLL_SECONDS`) the viewer asks again with `If-None-Match: "<pub_seq>"`. A 304 changes nothing; a new
   publication is imported into a **fresh** document (a shallow snapshot cannot be merged into an older one) and the page is
   drawn again where the reader was. A 429 doubles the wait (up to 32 times); a picture whose chunk the server does not hold
   yet shows as unavailable and is asked for again at the next poll.
5. States: the link is revoked, expired or unknown (404: one screen, "This link no longer works", and a page that was shown
   is replaced by it at once); the key is wrong or part of it was lost ("This link does not open"); no key in the address;
   offline (the page stays, with a notice, and refreshes when the server is back). English, French and Spanish, following
   the browser's language (`src/messages.ts`; the editor's own strings are the UI's).

The viewer shows exactly one page. Everything that would write is refused with `Forbidden`, and so is every request the
page view does not need.

## Build and serve

```text
. .\scripts\dev-env.ps1                          # this machine: cargo, perl, nasm, pnpm on PATH
pnpm --filter @atomik-kb/viewer build            # kb-wasm -> src/wasm, tsc, vite build -> apps/viewer/dist, check-dist
KB_SERVER_VIEWER_DIR=apps/viewer/dist  kb-server # serves /view, /view/ and /view/assets/*
```

`scripts/smoke/server.ps1` sets `KB_SERVER_VIEWER_DIR` itself when `apps/viewer/dist/index.html` exists. Without the
variable `GET /view` is 404 and nothing else changes (`deploy/kb-server/README.md`).

What the build needs: the `wasm32-unknown-unknown` Rust target (`rust-toolchain.toml`) and `wasm-bindgen-cli` in exactly the
version of the `wasm-bindgen` crate in `Cargo.lock`: `cargo install wasm-bindgen-cli --version 0.2.129 --locked`. No clang
(kb-wasm is `kb-crypto` and `kb-sync` only).

**The JavaScript glue is generated, not committed** (`src/wasm/`, ignored by git and eslint): `scripts/build-wasm.mjs` runs
`cargo build --release --target wasm32-unknown-unknown -p kb-wasm` and `wasm-bindgen --target web` on every `pnpm build`, so
the module in `dist` is always made from the Rust in the tree; a committed copy could drift from `kb-sync`'s codec. `test`,
`typecheck`, `lint` and `dev` build it only when it is missing (`--if-missing`); `pnpm build:wasm` makes it again.

The output of `vite build` is under `base: "/view/"`: `index.html` and `assets/<name>-<hash>.<ext>` (hashed, so the server may
cache them as immutable), the two WebAssembly modules (`kb_wasm_bg-*.wasm` and Loro's), and one chunk per language of the
editor. `scripts/check-dist.mjs` (the last step of the build, and `src/dist.test.ts`) checks the result against the policy.

`pnpm dev` serves `http://localhost:1430/view/#<key>` with the same policy as a `<meta>` tag and proxies `/v2` to a local
kb-server (`http://127.0.0.1:8787`).

## The policy

`default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; img-src 'self' blob:` (`VIEWER_CSP`), with
`Referrer-Policy: no-referrer` and `X-Content-Type-Options: nosniff`: kb-server sends them as headers and `index.html` as a
`<meta>` tag, word for word (`src/viewer.test.ts` compares them). There is no inline script and no inline style in the page.
Consequences that are decided, not bugs:

- The editor's own base rules come from a constructable stylesheet (`installEditorStyles`), not Tiptap's `<style>` element.
  `style` properties set by components are CSSOM and fine; markup that carries a `style` attribute is refused.
- Content that needs more than the policy allows shows without it: audio and video blocks (their `blob:` source is `media-src`),
  the inline styles of KaTeX's HTML and of Mermaid and Excalidraw drawings, and pictures embedded as `data:` addresses.
- Pictures are `blob:` URLs made from the decrypted bytes (`img-src blob:`).

## Tests

`pnpm --filter @atomik-kb/viewer test`: the transport against a fake server with the **real** `kb-wasm` module and real
fixtures (no request carries the key, the poll and its `ETag`, every state, ranges and tampered chunks), the views, the words in
three languages, the policy of `index.html`, and the build output when `dist/` exists.

`fixtures/` is made by `cargo run -p kb-wasm --example viewer_fixture` (a page with a heading, a mention, bold text and a
picture that is also its cover, sealed by `kb-sync`'s codec under the key `00 01 .. 1f`, a second publication of the same link
with one more paragraph, and the picture's one sealed chunk); existing files are never rewritten. `kb-wasm`'s own tests
(`cargo test -p kb-wasm`) open them natively too, so a codec change that breaks them fails there first.
