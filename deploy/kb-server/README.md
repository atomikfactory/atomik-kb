# kb-server

The sync server of atomik-kb. It stores and relays what
devices send it: encrypted, signed entries, snapshots and chunks, signed heads, each
account's device log (public keys, encrypted device names), recovery blobs (openable
only with the recovery kit) and short-lived pairing mailboxes (opaque messages). It
holds no key, cannot read content, titles or structure, and enforces only
authentication, idempotency, quotas, rate limits and pruning. One binary, one SQLite
file (`server.db`), no TLS of its own: put it behind a reverse proxy such as Caddy.

## Running it

With Docker Compose and Caddy (this directory):

```sh
export KB_DOMAIN=sync.example.org              # a DNS record pointing at this host
export KB_INVITE_TOKENS=$(openssl rand -hex 24)
docker compose up -d --build
```

Caddy obtains the certificate (ports 80 and 443 must reach the host) and proxies to
kb-server on the compose network; kb-server is never published directly.

For a home server reachable only over LAN / Tailscale (no domain, no Caddy), deploy from
Windows with `scripts/deploy/server.ps1` (uses `docker-compose.lan.yml`); see the comments at the
top of that script. Natively on Windows: `scripts/deploy/server-windows.ps1`.

The image alone: `docker build -f deploy/kb-server/Dockerfile -t atomik-kb-server .`
from the repository root. Without Docker: `cargo build --release -p kb-server`, then run
`target/release/kb-server` with the environment below, behind any TLS proxy.

## Environment

| Variable | Default | Meaning |
|---|---|---|
| `KB_SERVER_LISTEN` | `127.0.0.1:8787` (image: `0.0.0.0:8787`) | Address and port to listen on (plain HTTP). |
| `KB_SERVER_DATA` | `data` (image: `/data`) | Directory of `server.db`; created when missing. |
| `KB_SERVER_INVITE_TOKENS` | none | Comma-separated tokens that may create accounts. Empty: no account can be created (existing ones keep working). |
| `KB_SERVER_ACCOUNT_QUOTA_BYTES` | `10737418240` (10 GiB) | Bytes one account may store (entries, snapshots, chunks, heads, its device log). Beyond it writes answer 507. |
| `KB_SERVER_PRUNE_GRACE_DAYS` | `7` | Days after a snapshot arrives before the entries it covers are deleted. |
| `KB_SERVER_TRUSTED_PROXIES` | unset (none; compose: Caddy's address `172.30.0.10`) | Comma-separated addresses or CIDR networks (`172.30.0.10`, `10.0.0.0/8`, `fd00::/64`) of reverse proxies. For a request from one of them, the rate limits count the client named by `X-Forwarded-For` (the last address that is not a trusted proxy), else `X-Real-IP`, else the proxy itself. From any other peer these headers are ignored. |
| `KB_SERVER_IDLE_DEVICE_DAYS` | unset (off) | Refuse authentication to a device whose last session is more than this many days old (a positive integer). The device must then be paired again. |
| `KB_SERVER_SHARE_SIGNUPS` | `20` | Accounts that share invites may create on this server, counted over all time (not per time window). `0` switches share signups off; invites for accounts that exist keep working. Beyond it `POST /v2/accounts` with a share claim answers 403 `signup_limit`. |
| `KB_SERVER_SHARE_SIGNUPS_PER_ACCOUNT` | `5` | Accounts the invites of one inviting account may create (counted against the inviter). Beyond it, 403 `signup_limit`. |
| `KB_SERVER_VIEWER_DIR` | unset | The directory of the web viewer's build (`apps/viewer/dist`, made by `pnpm --filter @atomik-kb/viewer build`) that `GET /view` serves: the page a view-only link opens. Unset, missing or without an `index.html`, every path of `/view` answers a plain 404 and everything else is as before. Details under "View links and the viewer" below. |
| `RUST_LOG` | `info` | Log filter (`tracing` syntax, e.g. `kb_server=debug,tower_http=info`). |

A malformed value stops the server at start with a message naming the variable (never
its value).

## The first account

1. Set `KB_SERVER_INVITE_TOKENS` (a long random string; several, comma-separated, if
   several people get accounts) and start the server.
2. In the app: Settings → Sync, enter the server address (`https://sync.example.org`).
   The app asks for the invite token only when the server does not know the account
   yet; it is used once and never stored.
3. Other devices of the same account join by pairing (Settings → Devices: the new
   device shows a code, an existing device types it, both confirm the same six
   digits) or with the recovery kit: they need no token.

The token only gates account creation; change or remove it at any time (restart the
container). An account is identified by the hash of its device log's first entry:
there are no passwords, e-mail addresses or user names. A device proves itself by
signing a server challenge with its device key; sessions last 24 hours.

## Backups

Everything is in `server.db` and its write-ahead log (`server.db-wal`, `server.db-shm`)
in the data directory. Copy them consistently:

- Online, with SQLite's backup API (the image has no shell, so from a helper container
  sharing the volume):

  ```sh
  docker run --rm -v kb-server_kb-data:/data -v "$PWD":/backup alpine \
    sh -c 'apk add --no-cache sqlite >/dev/null && sqlite3 /data/server.db ".backup /backup/server.db"'
  ```

  (the volume name is `<compose project>_kb-data`; `docker volume ls` shows it).
- Offline: `docker compose stop kb-server`, copy the three files together, start again.
  Never copy `server.db` alone while the server runs.

What a backup is for: devices keep everything they synced, so a server that loses its
data is rebuilt by the devices themselves (they see a new server epoch and upload their
device log, keyring and a snapshot of every document of their personal feed; the owner
of a shared space posts the verified membership log of its feed again). A backup matters
for data that only the server still has, for example edits from a device that has since
been lost.

Restoring a backup older than what devices have already seen looks to them like a
rollback (the chain hash and the signed heads no longer match): they stop uploading
and show a warning, by design. To bring such devices back, start the server on an
empty data directory instead (a new epoch) and let the devices re-upload.

## Upgrades

- Back up `server.db` first, then `docker compose build --pull && docker compose up -d`
  (or replace the binary and restart).
- **A server must start fresh.** Wire protocol v2 replaced v1
  without a migration: the schema version is 3 (`PRAGMA user_version`), the sync tables
  are the `feed_*` ones, and a `server.db` written by a v1 server (schema version 1 or 2,
  or an `entries` table) is refused at start with `server.db holds protocol v1 data
  (schema version N); protocol v2 starts a fresh server: move the data directory
  away and start again`. Move or delete the data directory (the compose volume, or
  `scripts/smoke/server.ps1 -Action reset` for the local one) and start: the server picks a
  new epoch, and every device uploads again what it holds (an app of this version does so by
  itself; a device on a v1 build refuses the server, since `GET /v2/server` answers
  protocol 2 and `/v1/...` no longer exists).
- From schema version 3 on, a newer server upgrades an older file in place on start. A
  server refuses to open a database written by a newer version, so a downgrade fails
  loudly instead of damaging data; restore the backup taken before the upgrade to go
  back.
- The wire protocol is version 2 (`GET /v2/server` answers it). Devices refuse a server
  speaking another version.

## Operations

- Logs: one line per request with method, path, status and latency, plus the account
  and device ids once a request is authenticated. Never bodies, query strings, tokens,
  nonces or headers; a recovery id or a chunk hash in a path is cut to its first 8 hex
  characters (pairing mailbox ids are logged: they expire in ten minutes).
- Pruning runs at start and every hour: the payload of an entry is deleted once a
  snapshot of its document that covers it is older than the grace period (a small
  record of the entry stays, so a device re-sending it is answered as a duplicate and
  the chain hash at that point is kept). A shared feed also waits for every active
  device of every member to have a head past the cover (below). Keyring and membership
  entries are never pruned. Expired challenges, sessions and pairing mailboxes go too.
- Rate limits (in memory, reset by a restart): 10 pairing mailboxes per client address
  per 10 minutes, 60 requests per mailbox per minute, 5 recovery blob reads per
  address per minute, 120 public view-link reads per address per minute (a link's page
  and its attachment chunks together); beyond them the server answers 429. The client address is the
  TCP peer, or, when the peer is listed in `KB_SERVER_TRUSTED_PROXIES`, the client
  address the proxy forwarded (IPv6 counted per /64). Behind a proxy that is not
  listed every client shares the proxy's address, so one client could exhaust the
  limits for everyone: list the proxy. List only proxies you run (anyone else could
  name any address in those headers). The compose example gives Caddy the fixed
  address `172.30.0.10` on its own network and lists it; change the subnet, Caddy's
  address and `KB_TRUSTED_PROXIES` together if `172.30.0.0/24` is taken on your host.
  Header values are never logged.
- Idle devices (`KB_SERVER_IDLE_DEVICE_DAYS`): the server records when each device last
  opened a session; a device past the limit gets 401 `{"error": "idle"}` at its next
  authentication (a device never seen counts as seen at that moment; sessions already
  open run to their end). The refusal is the server's only: nothing is written to the
  device log.
- Health check: `GET /v2/server` answers 200 without authentication.
- Shutdown: SIGTERM or SIGINT finish the requests in flight and close the database.

## View links and the viewer

A view-only link is `https://<server>/view#<key>`: it shows one page, read-only,
to someone with no account. The key is in the URL fragment, which the browser never sends, so the server
stores a sealed publication of the page and the sealed chunks of its attachments and cannot read either.
The server that serves the viewer's script is trusted for that one page (a hostile operator could alter
the script), so serve it only from a server you run.

- **`KB_SERVER_VIEWER_DIR`** is the directory of the viewer's build, not something compiled into the
  binary (building the viewer needs node and the wasm toolchain; `cargo build -p kb-server` needs
  neither). Build it with `pnpm --filter @atomik-kb/viewer build` and point the variable at
  `apps/viewer/dist`; in a container mount or copy that directory and set the variable to its path.
  Unset, missing or without an `index.html`: `/view` answers a plain 404 and links do not open, but
  members can still create them and the `/v2/links` routes work. Only files below the directory are
  served (no listing, no `..`, no symlink out of it).
- **Headers** on every file of `/view`, the 404 too: `Content-Security-Policy: default-src 'self';
  script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; img-src 'self' blob:`, `Referrer-Policy:
  no-referrer`, `X-Content-Type-Options: nosniff`, and `application/wasm` for the module. A reverse proxy
  must pass them on unchanged and must not add a script, a style or a tracker to the page.
- **Limits**: a publication is at most 16 MiB (413 `too_large`; the client reports it as over the
  limit), declares at most 1024 attachment chunks, and expires after at most 365 days (the person
  chooses 1 to 365 days or none; the server refuses a longer one and deletes the link at the hourly
  pass once it has expired; an expired link answers 404 from the moment it expires). Public reads are
  limited to 120 per client address per minute (429 `rate_limited`): a viewer polls once a minute, so
  behind a proxy list it in `KB_SERVER_TRUSTED_PROXIES` or all your viewers share one address. The
  publication's bytes count against the quota of the account that pays for the space.
- **What the server learns**: link ids, publication sizes, the hashes of the declared chunks, which
  account created a link, and viewers' addresses and polling times. Closing a space deletes its links;
  removing a member does not (an Owner revokes the link in the app).
- **Logs** hold the first 8 hex characters of a link id and of a chunk hash, nothing of the publication
  and never the key (it does not reach the server).

## HTTP API (`/v2`)

Protocol v2 is the only protocol: `/v1` is gone. Metadata is camelCase
JSON; binary fields inside JSON are lowercase hex (epochs, nonces, signatures, chain hashes, chunk ids)
or standard base64 with padding (device log and membership entries, heads); ids are UUID strings.
Payloads (entries, snapshots, chunks, heads, pages) are raw `application/octet-stream` bodies. Errors
answer `{"error": "<code>", "message": "<text>"}`.

Authenticated calls carry `Authorization: Bearer <token>`. Rate-limited calls answer 429
`{"error": "rate_limited"}`. Missing, unknown or expired token, or a device no longer active in the log:
401. An object of another account or signed by another device than the session's, or a signature that
does not verify: 403 (error code `authentication` for a bad signature). Bodies over 16 MiB: 413. Over the
account quota: 507.

A space that several accounts share has a **feed** with a random id; an account's own data lives in its
**personal feed** (the feed id is the account id). The server keeps, per shared feed, the signed
membership log, who belongs to it with which role (derived by running the same fold the clients run), and
the sealed items; it never sees names, space names, content or keys. A `204` answers a read of something
a member's feed does not hold, so a `404` always means "not a member, or no such feed".

Authorization (checked on every request):

- a **stranger** (never a member, an unknown feed, another account's personal feed): 404 `not_found`;
- a **member** needs Commenter for appends and snapshot puts, Editor for chunk puts and copies and
  snapshot disputes, Owner for chunk deletes, any role for reads and heads; a role that is too low:
  403 `forbidden`;
- a **removed or departed** account reads the membership log and the feed up to its own `Remove` or
  `Leave`; `changes` at or past it is 403 `forbidden`, every other call 404;
- a content append while the feed waits for a `Rekey` (after a `Leave`, or a device revocation in a
  member's log that no `Rekey` covers): 409 `rekey_needed`; an entry sealed under a key epoch below the
  feed's: 409 `stale_epoch`; any write to a closed feed: 409 `feed_closed` (reads stay for 30 days,
  then the feed is deleted); a lost compare-and-swap or an id collision: 409 `conflict`.

Other codes: 403 `authentication` (a bad signature), 403 `signup_limit`, 429 `rate_limited`
(`invites/{id}/open`: 30 per client address per hour; 20 Rekeys per account per feed per day), 413
`too_large`, 507 `quota` (a shared feed counts against its creator, then against the earliest-joined
remaining Owner).

| Method and path | Auth | Request | Success | Other answers |
|---|---|---|---|---|
| `GET /v2/server` | none | | 200 `{protocolVersion: 2, serverEpoch, maxWireBytes, accountQuotaBytes, idleDeviceDays, shareSignups, shareSignupsPerAccount}` (`idleDeviceDays`: number or null) | |
| `POST /v2/accounts` | token or share claim | `{inviteToken, log: [base64 entry, genesis first]}` or `{share: {inviteId, signature: hex}, log}` (signature: the invite key over `"signup" ‖ account ‖ serverEpoch`) | 201 `{accountId}` | 401 wrong token; 404 dead invite; 400 invalid log; 403 `authentication`; 403 `signup_limit` (limits, second signup with one invite, a bound invite and another account); 409 account or device id exists |
| `POST /v2/accounts/{id}/log` | none (entries are signed) | `{entries: [base64]}` (`log` accepted as the field name) | 204 | 404 unknown account, 409 does not continue the stored chain or a device id another account uses, 400 invalid entry, 403 bad signature |
| `GET /v2/accounts/{id}/log[?after=seq]` | none | | 200 `{entries: [base64]}` after `seq` | 404 unknown account |
| `POST /v2/accounts/logs` | none | `{accounts: [{id, after?}]}` (at most 256) | 200 `{logs: [{account, entries}]}` (unknown accounts left out) | 400 |
| `POST /v2/auth/challenge` | none | `{accountId, deviceId}` | 200 `{nonce, serverEpoch}` (nonce: 32 bytes, valid 120 s, single use) | 404 unknown account, 401 device not active, 401 `idle` |
| `POST /v2/auth/session` | none | `{accountId, deviceId, nonce, signature}` (Ed25519, 64 bytes, over `nonce ‖ serverEpoch`, context `auth`) | 200 `{token, expiresAt}` (Unix ms, 24 h) | 401 unknown/used/expired challenge, other device, inactive device, bad signature; 401 `idle`; 400 malformed |
| `POST /v2/feeds` | bearer | a `Create` membership entry (octet-stream) | 201 `{feedId, head: {seq, hash}, cursor}` | 409 feed id taken (also an account id); 400 not a `Create` or a rule broken; 403 `authentication`; 403 `forbidden` over 100 feeds |
| `GET /v2/feeds` | bearer | | 200 `{feeds: [{feedId, kind, role, removed, closed}]}` | |
| `POST /v2/feeds/{f}/membership` | bearer | `{entries: [base64]}` | 200 `{seq, hash}` of the new head | 409 `conflict` (the compare-and-swap lost, or a device log the entry names is not on the server yet), 403 `forbidden` (a role), 409 `feed_closed`, 400 (a rule broken), 404 (a non-member whose batch is not its own valid `Join`), 429 (Rekey rate) |
| `GET /v2/feeds/{f}/membership?after=seq` | bearer | | 200 `{entries: [{cursor, bytes}], head, more}` | 404 |
| `POST /v2/feeds/{f}/entries` | bearer | entry or keyring envelope of that feed, the session's account and device | 200 `{cursor, chainHash}` (a duplicate answers the stored values) | 403 `forbidden`/`authentication`, 409 `rekey_needed`/`stale_epoch`/`feed_closed`, 404, 507, 400 |
| `GET /v2/feeds/{f}/changes?after=N&limit=M` | bearer | | 200 changes page (binary; membership entries included) | 403 for a removed account at its `Remove`, 404 |
| `PUT /v2/feeds/{f}/snapshots/{doc}` | bearer | snapshot envelope | 204 | 409 covers less than the stored one, as `entries` |
| `GET /v2/feeds/{f}/snapshots[/{doc}]` | bearer | | 200 page / bytes; 204 for no snapshot of `doc` | 404 |
| `GET /v2/feeds/{f}/snapshot-marks?after=N` | bearer | | 200 `{marks: [{mark, docId, deviceId, accountId, coversCursor}], more}` | 404 |
| `POST /v2/feeds/{f}/snapshots/{doc}/dispute` | bearer (Editor) | `{coversCursor}` | 204 (entries the snapshot covers are kept until a newer snapshot replaces it) | 404 no snapshot |
| `PUT /v2/feeds/{f}/heads`, `GET /v2/feeds/{f}/heads` | bearer | signed head naming this feed, the session's account and device, a cursor the feed has reached | 204; 200 `{heads}` | 400, 403, 409 `feed_closed` |
| `PUT/GET/DELETE /v2/feeds/{f}/chunks/{hash}` | bearer | | 204; 200 bytes or 204 none; 204 (Owners only) | 400 hash mismatch, 403, 404, 507 |
| `POST /v2/feeds/{f}/chunks/{hash}/copy` | bearer (Editor) | | 204 (from the session account's personal namespace) | 404 no such chunk there |
| `POST /v2/invites/{id}/open` | none | `{signature: hex}` (the invite key over `"open" ‖ inviteId ‖ serverEpoch`) | 200 `{feedId, log: [{cursor, bytes}]}` | 404 (unknown, expired, used, revoked, closed feed, bad signature: one answer), 429 |
| `PUT /v2/links/{id}` | bearer (Editor or Owner of the feed; the account itself for its personal feed) | upload (octet-stream): `u8 2 ‖ feed ‖ u8 flags ‖ i64 expiresAt ‖ u32 n ‖ n × [32] chunk hash ‖ sealed publication`; the publication's header must name `{id}` | 200 `{pubSeq}` (1 at the first upload, +1 each time; the first upload fixes the creator and the expiry) | 404 stranger, 403 `forbidden` (Viewer, Commenter), 409 `feed_closed`, 409 `conflict` (the link belongs to another feed), 400 (malformed, over 1024 chunks, expiry over 365 days, another link's publication), 413 over 16 MiB, 507 |
| `GET /v2/links/{id}` | none | `If-None-Match: "<pubSeq>"` optional | 200 sealed publication with `ETag: "<pubSeq>"` and `Cache-Control: no-store`; 304 when it matches | 404 unknown, expired or revoked (one answer), 429 (120 reads per address per minute) |
| `DELETE /v2/links/{id}` | bearer (the creator while still a member, or an Owner) | | 204 (the row, the publication and the declared chunks go) | 403 another member, 404 stranger or unknown |
| `GET /v2/links/{id}/chunks/{hash}` | none | | 200 sealed chunk the link declared and its feed holds | 404 (anything else), 429 (counted with the page reads) |
| `GET /view`, `GET /view/{path}` | none | | 200 a file of `KB_SERVER_VIEWER_DIR` with the headers above | 404 (anything that is not a file below the directory, or no viewer) |
| `GET /v2/ws` | bearer header or `?token=` | WebSocket upgrade | 101; text frames `{"type":"changed","cursor":N}` (a feed the account may read grew; N is the newest cursor) | 401; 400 without an upgrade |
| `POST /v2/pair` | none | `{offer: base64}` (1 to 16 KiB) | 201 `{mailboxId: 12 lowercase hex, expiresAt: Unix ms}` (message 0 = the offer, role `joiner`; lives 10 min) | 413 over 16 KiB, 400 malformed, 429 (10 per address per 10 min) |
| `GET /v2/pair/{mailboxId}/messages?after=N&wait=S` | none | `after`: messages the caller already has (default 0); `wait`: seconds, default 0, at most 25 | 200 `{messages: [{seq, role: "joiner"\|"sponsor", bytes: base64}], expiresAt, closed}`: the messages with `seq >= after` | 404 unknown or expired, 400 malformed, 429 (60 per mailbox per minute) |
| `POST /v2/pair/{mailboxId}/messages` | none | `{role: "joiner"\|"sponsor", bytes: base64}` (1 to 16 KiB) | 204 (next sequence) | in this order: 404 unknown or expired, 400 bad role or base64, 413 over 16 KiB, 409 closed or 64 messages; 429 |
| `DELETE /v2/pair/{mailboxId}` | none | | 204 (closes; closing twice is fine) | 404 unknown or expired, 429 |
| `PUT /v2/recovery/{recoveryId}` | bearer | the blob (1 byte to 4 KiB, 64-hex id) | 204 (stored with the session's account, replaces) | 403 the id belongs to another account, 413 over 4 KiB, 400 malformed |
| `GET /v2/recovery/{recoveryId}` | none | | 200 blob bytes | 404, 400 malformed, 429 (5 per address per minute) |
| `DELETE /v2/recovery/{recoveryId}` | bearer | | 204 (also when nothing is stored) | 403 another account's blob |
| `GET /join`, `/join.js`, `/join.css` | none | | the static page of the link flow (strict CSP, no referrer, no caching); outside `/v2` | |

A changes page carries at most 200 entries and at most 16 MiB of entry bytes (always at
least one entry when one follows); `more` says whether the feed continues. Its chain hash
is the one after its last entry, or after `after` when it is empty. On the WebSocket the
server sends one frame per new cursor (frames queued while one is sent are merged into
the newest), pings every 30 s, closes a connection silent for 75 s, and closes it when
the session expires.

A pairing mailbox answers a long-poll as soon as a message with `seq >= after` exists,
when it is closed, or when `wait` elapses (an empty `messages`); one that expires while
a poll waits answers 404. A closed mailbox stays readable, with `closed: true`, until
it expires. Recovery blobs do not count toward the account quota.

Chunks (the encrypted pieces of attachments) count toward the quota until a device deletes
them with `DELETE /v2/feeds/{feed}/chunks/{hash}` (on the personal feed any active device of
the account, on a shared feed an Owner). The server never deletes a chunk on its own: it cannot read the
manifests that reference them, so a device that purges an attachment deletes its chunks. Storing a chunk
again after a delete counts it again.

**Pruning.** A document's entries up to a snapshot's cursor are deleted only when the snapshot is
`KB_SERVER_PRUNE_GRACE_DAYS` old (7 days) and nobody disputed it; on a shared feed every active device of
every member that is not idle must also have a head at or past that cursor (the personal feed waits for
the grace period only). A device is idle when
its last session is older than `KB_SERVER_IDLE_DEVICE_DAYS`, or 30 days when that is unset. Membership
and keyring entries are never pruned.

**Logging.** Paths only, every id cut to its first 8 hex characters (including invite ids and chunk
hashes), no query strings, no bodies, no tokens; membership changes are logged as counts.

**Reset.** There is nothing to migrate: delete the data directory (a new server epoch) and the devices
re-publish; members re-post the verified membership log of each shared feed. A data directory of a
protocol v1 server is refused at start (see Upgrades).
