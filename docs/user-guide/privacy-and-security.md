# Privacy and security

This chapter explains in plain language what atomik-kb protects, from whom, and where the
protection ends. A privacy tool is only trustworthy if it is honest about its limits.

## The short version

- Your notes are **encrypted on your device** and stay there unless you turn on sync,
  backups to Google Drive, or a remote AI address.
- If you sync, the server stores only **ciphertext**. The person who runs it cannot read
  your pages, titles, structure, tags, properties, comments or file names.
- **Nobody can reset your access.** Your recovery kit is the safety net. Lose every device
  and the kit and the notes are unrecoverable.
- Encryption protects data at rest and in transit. It does not protect a computer that is
  already compromised.

## What is encrypted, and how

- **On disk:** the database is encrypted (SQLCipher), and so is everything inside it:
  pages, search index, attachments and settings. The key is kept in your operating
  system's credential store (Windows Credential Manager, or the Secret Service on Linux).
  A stolen powered-off or locked computer reveals nothing without that key.
- **App lock (optional):** adds a passphrase that wraps the key (Argon2id), so the notes
  are closed until you type it. It is the only password-derived secret in the system.
- **Every page, every file, its own key:** pages and attachments have separate keys, so
  deleting one for good removes its key and makes any leftover copy unreadable.
- **In transit and on the server:** updates are encrypted and signed on your device before
  upload. Modern, widely reviewed primitives are used (XChaCha20-Poly1305, Ed25519, X25519,
  BLAKE3, HKDF, Argon2id).
- **No plaintext in the web view:** the app's interface never receives keys, has no remote
  content, and runs under a strict content security policy.

## What the sync server can see

Even though it cannot read content, a server operator can see **metadata**: that an
account exists, how many devices it has, how much you store, when you sync, the IP
addresses that connect, and the size of each upload (padded to coarse sizes). For shared
spaces it also sees which accounts are members, their roles, and when people join or are
removed. It never sees names, space names, content, comments, or which page anyone is on.

A dishonest server can refuse service or hide data, but it cannot forge your edits or quietly
rewrite history. Devices check signatures and detect rollbacks and forks; when something is
wrong, sync **pauses with a warning** instead of overwriting your notes.

## Devices and pairing

A new device joins through a code **and six digits you compare on both screens**. This
stops a malicious server from inserting its own device into your account. Never confirm digits
over a channel you cannot trust, and nobody from atomik-kb will ever ask you for them.
Removing a device rotates your keys so that it cannot read anything new.

## Sharing

- Members of a shared space see what their role allows; roles are enforced by every member's
  app, and the server refuses reads by non-members.
- **A removed member keeps what they already received.** Encryption cannot take back
  data that someone has already seen. Everyone invited also receives the space's full
  history.
- **A hostile Owner** can remove everyone else and close the space. Only invite Owners you trust.
- **Safety numbers** let you confirm that you are really talking to the person you think.
- **View links** are different: anyone with the link can read that one page, and the
  server that serves the viewer can read it too.

## Local AI, MCP and Google Drive

- Local AI keeps text on your computer unless you point it at a remote Ollama address; then
  the app labels the setting **Remote: text leaves this device**.
- Programs you connect to the MCP server can read whatever you grant them. Each has its
  own revocable key and an activity log.
- Google Drive backups are encrypted before upload; Google sees only names, sizes and
  timestamps.
- The app has no analytics and no telemetry.

## What atomik-kb does not protect against

- **Malware or another user on your unlocked computer.** Auto-lock reduces exposure but
  cannot prevent a compromised machine from reading what you can read.
- **Past data after a device is stolen.** Revoking a device protects the future, not the
  copies it already holds.
- **Metadata** such as the fact that you use a server, how much data you store and when
  you sync.
- **People who copy what they can read.** Members and link holders can copy, export or
  screenshot.
- **Forgetting your recovery kit and losing your devices.**
- **Deniability** and Signal-style forward secrecy for stored notes: stored data must be
  readable later, so it cannot have per-message forward secrecy.

The Windows installer and the macOS app are not code-signed yet, and the software is early (version 0.1) and
has not had an independent security audit. Keep backups, and treat it accordingly.

## Good habits

1. Save the recovery kit on paper, in a safe place, and test it once.
2. Turn on the app lock on laptops and shared computers.
3. Keep your sync server updated and back up its data folder.
4. Use HTTPS or a private network such as Tailscale for the server.
5. Verify safety numbers with the people you share with.
6. Treat view links and invitation links like passwords.
