# atomik-kb

A privacy-first, local-first knowledge base: a Notion-style workspace whose pages are
encrypted on your own devices and synchronised, if you want, through a server that
cannot read them.

## Features

- **Pages and blocks.** Rich-text pages with nested sub-pages, headings, lists, to-dos,
  toggles, callouts, code, equations, tables, columns, tables of contents, images, photo
  galleries, PDFs, audio and video, file attachments, diagrams and whiteboards. Links and
  mentions between pages, with backlinks.
- **Organise.** Spaces, a page tree, tabs, favourites, recent pages, tags, trash with
  retention, full-text search across spaces, find in page, and page icons and covers.
- **Databases.** Typed properties (text, number, select, status, date, checklist, files,
  place, person, relation, rollup, formula and more) and views over them: list, table,
  board, calendar, gallery, feed and dashboard, with filters, multi-level sorts, bulk
  edit, templates, and Notion-style formulas.
- **Templates and history.** Page templates with placeholders, template buttons, and a
  page history with highlighted changes and whole-page restore.
- **Sync across devices.** Optional, through a self-hosted server. End-to-end encrypted;
  devices are paired with a short code and six confirmation digits.
- **Shared spaces.** Invite other people to a space with a role (Owner, Editor,
  Commenter, Viewer), comment threads, @mentions, live presence, and read-only web links
  to single pages.
- **Backup and recovery.** A 24-word recovery kit, encrypted backup files, scheduled
  backups to a folder and to Google Drive (still encrypted), and a restore wizard.
- **Import and export.** Markdown, HTML, CSV, text, Notion exports and AppFlowy backups
  in; Markdown, HTML, PDF (through the print dialog) and CSV out.
- **Local AI, off by default.** Writing help, questions about your notes and transcription
  through [Ollama](https://ollama.com), local dictation, and an optional local MCP
  server for MCP-compatible tools.
- **Languages.** English, French and Spanish.

## Privacy and security in brief

- Your notes live in an encrypted database on your device. Keys are held in the operating
  system's credential store; an optional passphrase locks the app.
- Pages are CRDT documents ([Loro](https://loro.dev)), so devices can edit offline and
  merge without conflicts.
- The sync server is *zero-knowledge*: it stores and relays encrypted, signed data and
  never holds a key. It cannot read titles, content, structure or comments. It does see
  metadata such as sizes, timing and IP addresses.
- There are no passwords or e-mail addresses on the server. A device proves itself with
  a signing key.
- The recovery kit is the only way back if every device is lost. Lose it and your
  notes cannot be recovered by anyone, including us.
- Nothing is sent anywhere unless you turn on sync, Google Drive backups or AI with a
  remote Ollama address.

Details and honest limits: [docs/user-guide/privacy-and-security.md](docs/user-guide/privacy-and-security.md).

## Status

Early software (version 0.1). The desktop app is the main product and is under active
development; expect rough edges and keep backups.

| Component | State |
|---|---|
| Desktop app (Tauri + React) | Working: Windows installer and Linux `.deb` can be built from this repository |
| Sync server (`kb-server`) | Working, self-hosted (Docker or a plain binary) |
| Web viewer | Read-only viewer for view links; not a full web client |
| macOS | Not packaged or tested |
| Android | Not started in this repository |

## Install

There are no published release builds yet. Build from source (below) to get the app.
The Windows installer is unsigned, so SmartScreen shows a warning on first run. On
Linux the app needs a Secret Service (for example gnome-keyring or KWallet) running in
your session.

Start with the [user guide](docs/user-guide/README.md).

## Building from source

Requirements:

- Rust, pinned by `rust-toolchain.toml` (currently 1.99.0, with the `wasm32-unknown-unknown`
  target); `rustup` installs it automatically.
- Node.js and [pnpm](https://pnpm.io).
- Windows: Visual Studio C++ build tools and the WebView2 runtime. Linux: the usual Tauri 2
  system libraries (WebKitGTK and friends).
- The first build of the storage crate compiles SQLCipher and OpenSSL from source, which
  needs a C toolchain, Perl and NASM (Windows), and takes several minutes.

```sh
pnpm install

# Rust core
cargo build --workspace
cargo test -p kb-core            # or any crate: cargo test -p <crate>
cargo fmt --all
cargo clippy --all-targets

# The core crates also compile to WebAssembly
cargo check --target wasm32-unknown-unknown -p kb-types -p kb-crypto -p kb-keys \
  -p kb-doc -p kb-model -p kb-sync -p kb-wasm

# Front end
pnpm lint && pnpm typecheck && pnpm test && pnpm build

# Desktop app in development mode
cd apps/desktop && pnpm tauri dev
```

Installers are produced with `pnpm tauri build` in `apps/desktop`. The helper scripts in
`scripts/package` (`windows.ps1`, `linux.ps1`) wrap that, including the dictation helper
program and its libraries.

## Repository layout

| Path | What it is |
|---|---|
| `crates/kb-types` | Ids, the core API contract and the platform traits |
| `crates/kb-crypto` | Cryptographic primitives and envelopes |
| `crates/kb-keys` | Key hierarchy, recovery kit, device log, app lock |
| `crates/kb-storage` | Encrypted SQLite (SQLCipher) schema and queries |
| `crates/kb-doc` | CRDT engine boundary over Loro |
| `crates/kb-model` | Typed documents: spaces, page tree, properties |
| `crates/kb-core` | The core: one API surface that every app talks to |
| `crates/kb-sync`, `crates/kb-net` | Sync protocol (pure) and its network client |
| `crates/kb-server` | The zero-knowledge sync server |
| `crates/kb-import`, `crates/kb-export` | Importers and exporters |
| `crates/kb-ai`, `crates/kb-mcp` | Local AI rules; MCP server and bridge |
| `crates/kb-speech` | Local dictation rules |
| `crates/kb-drive` | Google Drive backup protocol |
| `crates/kb-wasm` | WebAssembly module of the web viewer |
| `adapters/` | Native implementations: storage, speech recogniser |
| `apps/desktop` | Tauri desktop app |
| `apps/viewer` | Web viewer for view-only links |
| `packages/ui` | Shared React UI and editor |
| `packages/core-client` | Typed TypeScript client of the core API |
| `deploy/kb-server` | Dockerfile and compose files for the server |
| `scripts/` | Packaging, deployment and verification scripts |

## Self-hosting the sync server

Sync is optional. To run your own server you need one small machine and a way for your
devices to reach it (a domain with HTTPS, or a private network such as Tailscale).

```sh
cd deploy/kb-server
export KB_DOMAIN=sync.example.org               # DNS record pointing at this host
export KB_INVITE_TOKENS=$(openssl rand -hex 24) # lets you create your account
docker compose up -d --build
```

This starts the server behind Caddy, which obtains a TLS certificate. Without Docker:
`cargo build --release -p kb-server`, then run `target/release/kb-server` behind any TLS
proxy. Configuration is by environment variables (listen address, data directory,
invite tokens, quotas); see [deploy/kb-server/README.md](deploy/kb-server/README.md) and
the user guide's [sync and devices](docs/user-guide/sync-and-devices.md) chapter.

## Documentation

- [User guide](docs/user-guide/README.md)
- [Server deployment reference](deploy/kb-server/README.md)

## Contributing

Issues and pull requests are welcome. Before sending a change, run `cargo fmt`,
`cargo clippy --all-targets`, the tests of the crates you touched, and
`pnpm lint && pnpm typecheck && pnpm test`. The code base forbids `unsafe`, and
`unwrap`/`expect` outside tests; keep secrets out of logs and keep platform access behind
the existing traits. Please open an issue to discuss larger changes first.

## Licence

atomik-kb is free software, released under the
[GNU General Public License, version 3 or (at your option) any later version](LICENSE).
Unless you state otherwise, any contribution you submit for inclusion is licensed the
same way.
