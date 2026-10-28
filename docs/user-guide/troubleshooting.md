# Troubleshooting

## Start-up and unlocking

**"That passphrase is not right."** The app lock passphrase is case-sensitive. There is no
reset: if it is lost, restore your notes on a new installation from a backup with your
recovery kit (see [Backup and restore](backup-and-restore.md)).

**The app cannot unlock or open its keys on Linux.** atomik-kb keeps its keys in the
Secret Service (gnome-keyring, KWallet). Make sure one is installed, running and unlocked
in your desktop session, then start the app again.

**Windows warns when I run the installer.** The installer is not code-signed. Choose
**More info > Run anyway** if you built or downloaded it from a source you trust.

**macOS says the app is damaged or cannot be opened.** The app is not signed or notarized by
Apple. Right-click the app and choose **Open**, or run
`xattr -dr com.apple.quarantine /Applications/atomik-kb.app` once, if you downloaded it from a
source you trust.

**"A workspace already exists on this device."** Restoring a backup, recovering, or
joining an account works only on a new installation with no workspace.

## Recovery kit

**"This recovery kit does not open this backup."** Check the words and their order, or try
the code form. The first four letters of each word are enough. Backups made before you
re-issued the kit need the *old* kit.

**"This recovery kit is not valid" or "This server has no account for this recovery kit."**
Check the server address, and use the newest kit you have. After you remove a device or
re-issue the kit, older kits stop working.

**The kit screen says "kit already exists".** To get a new one, use **Settings > Security >
Recovery kit > Re-issue...**.

## Sync

**The status says Offline or Error.** Open **Settings > Sync**: it shows the last error.
Check that the server address is reachable from this computer (open
`<server address>/v2/server` in a browser: it should answer with a small JSON text) and
that the address uses the right scheme and port. Plain `http://` addresses work on a
private network; use `https://` across the internet.

**"Sync paused: the server's history does not match this device's."** A device noticed the
server presenting older or different history than it has already seen, for example after
the server was restored from an old backup or replaced. Uploads pause so nothing is
overwritten. Use **Export a backup** first, then either **Disconnect** and reconnect, or,
if you restored the server, start it on an empty data folder so that your devices re-upload.

**The server refuses to create my account.** It needs an invite token from its operator
(`KB_SERVER_INVITE_TOKENS`). Without any token configured, no new account can be made.

**The server will not start after an upgrade.** It refuses a database written by a newer
version, and a database from an older, incompatible protocol must be moved away so that
the server starts fresh (the devices upload their data again).

**A device stopped syncing after a long absence.** If the server uses
`KB_SERVER_IDLE_DEVICE_DAYS`, a device idle for longer must be paired again.

**An attachment will not open on a device.** With **Settings > Attachments > Download
attachments** set to **When opened**, press the file's **Download** button.

## Adding devices

**"This code expired or your sync server does not know it."** Pairing codes last ten
minutes. Start again on the new device.

**"The code has 12 letters and digits."** Re-type the code shown on the new device.

**"Connect this device to your sync server first."** On the existing device, set up
**Settings > Sync** before adding a device.

**The six digits differ.** Cancel. Do not confirm. This can mean a misdirected connection or an
attack.

**"Your keys were rotated on another device."** Enter your app-lock passphrase in the
banner to finish.

## Sharing

**I cannot create a view link.** Only Owners may, unless an Owner turned on **Editors may
create view links** in the space settings. Also check that the server serves the web viewer
(`KB_SERVER_VIEWER_DIR`); without it the link page does not open.

**A page opened from a link says it is unknown.** "Unknown page" means the page is not on
this device or is in the trash.

**"This link no longer works".** The view link was revoked or expired.

**A member's changes were refused.** Their role does not allow that change (a notice tells
you); change their role in **Members and sharing...**.

## AI, dictation and the microphone

**"Not connected" under Settings > AI.** Start Ollama, check the address, then **Check
again**. Models must be downloaded with Ollama first.

**No model to choose.** Install a chat model in Ollama, then reopen the settings.

**The microphone is blocked.** Allow microphone access for atomik-kb. If you chose Block
earlier, that choice is remembered. On Windows, the system privacy settings must also allow
desktop apps to use the microphone. A small permission window may be hidden behind the
main window.

**Dictation engines say "not available in this version".** The built-in speech engines
ship with the Windows installer; elsewhere use the **Ollama** engine, which needs a
transcription model in the AI settings.

**The MCP server will not start.** The port may be in use; choose another between 1024 and
65535. A tool that cannot see any page has no permissions yet: grant a space under
**Permissions**.

## Backups

**"Choose a folder first" / "Create your recovery kit first".** Scheduled backups need a
folder or Google Drive, and any backup needs your recovery kit.

**Google Drive says "needs you to sign in again".** Choose **Reconnect Google Drive**.

**"Your Google Drive is full".** Free some space, or lower **Backups to keep**.

**Restore says the file is damaged.** The file may not be an atomik-kb backup, or it was
cut short. Try another copy.

## Reading the diagnostics

**Diagnostics** in the main navigation shows technical details about this installation
(schema version, whether the database is encrypted, device ID, core and API versions,
platform, data folder). Nothing on that page leaves your device, and it is useful to copy
into a bug report. Do not share your recovery kit, passphrase or invite tokens with
anyone.
