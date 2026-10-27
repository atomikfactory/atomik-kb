# Sync and devices

Sync is optional. Without it, atomik-kb works fully offline on one computer. With it, your
devices stay up to date through a **sync server that you run (or trust)**. Your notes are
encrypted on your device before they leave it, so the server only stores data it cannot read.

## What the server can and cannot see

**It stores:** encrypted, signed updates and snapshots, encrypted attachments in
fixed-size chunks, the public keys of your devices, and a locked copy of your account that
only your recovery kit opens.

**It cannot read:** page titles, text, structure, tags, properties, comments, file names or
file types. It holds no key.

**It can see (metadata):** that an account exists, how many devices it has, roughly how
much data it stores and when it syncs, and the IP addresses that connect. If it is
dishonest it can refuse service or withhold data, but devices detect rollbacks and
forks and pause uploading with a warning rather than overwrite anything.

There are no passwords, e-mail addresses or user names on the server. An account is
identified by a key, and a device proves itself by signing a challenge.

## Run your own server

You need a small always-on machine (a home server, a Raspberry Pi-class box or a
cheap VPS) and a way for your devices to reach it. The server is one program with one
database file; it speaks plain HTTP and expects a reverse proxy to add HTTPS, or a private
network such as Tailscale or a LAN.

### Option A: Docker with automatic HTTPS (public server)

You need Docker with the compose plugin, a domain name pointing at the machine, and ports
80 and 443 open. From a copy of the repository:

```sh
cd deploy/kb-server
export KB_DOMAIN=sync.example.org
export KB_INVITE_TOKENS=$(openssl rand -hex 24)   # keep this value
docker compose up -d --build
```

Caddy gets and renews a certificate; the server itself is never exposed directly. Your
server address is `https://sync.example.org`.

### Option B: Docker on a LAN or Tailscale (no domain, no HTTPS)

Use `docker-compose.lan.yml` from the same folder. It publishes the server on a chosen
address and port (default `8787`) over plain HTTP, which is reasonable only on a private
network such as a LAN or Tailscale. Build the image first
(`docker build -f deploy/kb-server/Dockerfile -t atomik-kb-server .` from the repository
root), then copy the compose file to a folder of its own with a `.env` file containing
`KB_INVITE_TOKENS`, `KB_BIND` (the address to listen on) and `KB_PORT`, and run
`docker compose up -d`. Your notes remain end-to-end encrypted even over plain HTTP; HTTPS
or Tailscale additionally hides who talks to whom from the network.

The repository's `scripts/deploy/` folder also holds helper scripts that build and ship
the image to a Linux machine over SSH (`server.ps1`, PowerShell) and run the server as a
background task on Windows without Docker (`server-windows.ps1`). They are optional.

### Option C: the plain binary

```sh
cargo build --release -p kb-server
KB_SERVER_LISTEN=0.0.0.0:8787 KB_SERVER_DATA=/var/lib/atomik-kb \
KB_SERVER_INVITE_TOKENS=<your token> target/release/kb-server
```

Put it behind any TLS reverse proxy for internet use.

### Server settings

Set these as environment variables (in compose, most have a `KB_` form in the `.env`):

| Variable | Default | Meaning |
|---|---|---|
| `KB_SERVER_LISTEN` | `127.0.0.1:8787` | Address and port (the Docker image uses `0.0.0.0:8787`) |
| `KB_SERVER_DATA` | `data` | Folder holding `server.db` |
| `KB_SERVER_INVITE_TOKENS` | none | Comma-separated tokens that may create accounts; with none set, nobody new can sign up |
| `KB_SERVER_ACCOUNT_QUOTA_BYTES` | 10 GiB | Storage allowed per account |
| `KB_SERVER_PRUNE_GRACE_DAYS` | 7 | Days before data covered by a newer snapshot is deleted |
| `KB_SERVER_IDLE_DEVICE_DAYS` | off | Refuse devices that have not connected for this many days |
| `KB_SERVER_TRUSTED_PROXIES` | none | Reverse proxies whose forwarded client address is trusted for rate limits |
| `KB_SERVER_VIEWER_DIR` | unset | Folder of the web viewer build, needed for view-only links (see [Sharing](sharing.md)) |

The invite token only gates account creation. You can change or remove it at any time
by restarting the server with different settings; existing accounts keep working.
The full list, including the limits for shared-space invitations, is in
[deploy/kb-server/README.md](../../deploy/kb-server/README.md).

### Server backups

Everything is in `server.db` and its `-wal` and `-shm` files in the data folder. Stop the
server and copy all three together, or use SQLite's online backup. Never copy `server.db`
alone while it is running. Because your devices keep everything they synced, a server
that loses its data is rebuilt by the devices themselves; a backup matters mostly for
edits that only the server still holds. Restoring a server backup that is *older* than
what your devices have seen looks like a rollback to them, so they pause and warn you.
To recover from that, start the server on an empty data folder and let the devices
upload again. Back up before upgrading the server.

## Connect your first device

1. Open **Settings > Sync**, enter the server address (for example
   `https://sync.example.org` or `http://100.64.0.5:8787`) and choose **Connect**.
2. If the server does not know your account yet it asks for the **invite token**. Enter it;
   it is used once and not stored.
3. The status shows **Connected**, the last sync time and any changes waiting to upload.
   **Sync now** forces a cycle, and **Disconnect...** stops syncing (your notes stay on
   the device). A small sync indicator is always visible in the app.

Create your recovery kit first (see [Getting started](getting-started.md)): the list of
your devices appears only once the kit exists.

## Add another device

On the **new** computer, install atomik-kb and choose **Add this device to my account** on
the welcome screen. Enter your server address; the new device shows a **12-character code**.

On a device that already has your notes: **Settings > Security > Devices > Add a device...**
and type the code. Both screens now show **six digits**. Check they are identical and
confirm on both. Those digits are what stops a dishonest server from slipping its own
device in, so never confirm them for someone you cannot verify. The new device then
downloads your notes. The code works for ten minutes. No password is involved and your
notes never pass through anyone else.

## Lost every device?

On a fresh install choose **Recover with my kit**, type the server address and your 24
words. The server holds a copy of your account that only the kit opens. This works only
if the server still has your data; otherwise use a backup file (see
[Backup and restore](backup-and-restore.md)).

## Remove a lost or retired device

**Settings > Security > Devices > Remove...** next to the device. This rotates your keys
and issues a **new recovery kit** (the old one stops working), so have your app-lock
passphrase ready and save the new kit. The removed device cannot read anything new and
the server refuses it, but it keeps whatever it had already synced: revocation protects the
future, not the past. Your other devices finish the key rotation when you next open them
and enter your app-lock passphrase.

If a device joined using the recovery kit, the app asks you to re-issue the kit so that
the words typed there stop working.

## Attachments over sync

**Settings > Attachments > Download attachments** chooses **Automatically** (files of
synced pages arrive as soon as available) or **When opened** (a file comes only when you
press its Download button, which saves disk space).
