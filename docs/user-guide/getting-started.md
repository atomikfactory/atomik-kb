# Getting started

## Install

atomik-kb for desktop runs on **Windows**, **Linux** and **macOS** (Apple Silicon). There is no Android or full web version yet.

- **Windows:** run the setup program (`atomik-kb_<version>_x64-setup.exe`). It installs for
  your user only, without administrator rights. The installer is not code-signed, so
  Windows SmartScreen may warn you the first time: choose **More info > Run anyway**.
  Upgrading is done by running a newer setup program; your notes are not touched.
- **Linux (Debian/Ubuntu):** `sudo apt install ./atomik-kb_<version>_amd64.deb`. The app
  keeps its keys in your desktop's secret store, so a Secret Service such as
  gnome-keyring or KWallet must be running and unlocked in your session.
- **macOS (Apple Silicon, macOS 12 or newer):** open the `.dmg` and drag atomik-kb to
  **Applications**. The app is not signed or notarized by Apple, so macOS refuses to open it
  the first time ("cannot be opened because the developer cannot be verified", or "is damaged"
  for a downloaded file). To allow it, either right-click (or Control-click) the app and choose
  **Open**, then **Open** again; or choose **System Settings > Privacy & Security > Open
  Anyway**; or run `xattr -dr com.apple.quarantine /Applications/atomik-kb.app` in Terminal
  once. Intel Macs are not supported. Dictation is included (see AI and dictation).

There are no published downloads yet; the installers are built from the source code
(see the project README). Uninstalling does not delete your notes.

## First launch

1. **Welcome to atomik-kb.** Type a name for your workspace (for example "Personal") and
   choose **Create workspace**. A first space with that name is created for you.
2. Your notes are stored in an encrypted database on this computer. The encryption key is
   kept in the operating system's credential store, so you are not asked for a password.
   You can add an optional **app lock** later (see below).
3. Create your **recovery kit**. Do it now; it takes a minute.

Already have atomik-kb elsewhere? The welcome screen also offers:

- **Add this device to my account**: join through your sync server (see
  [Sync and devices](sync-and-devices.md)).
- **Recover with my kit**: lost every device, but you have the kit and a sync server.
- **Restore from a backup**: bring a backup file into this new installation (see
  [Backup and restore](backup-and-restore.md)).

## Your recovery kit

The recovery kit is a list of **24 words** (also available as a code). It is the one thing
that cannot be replaced.

**Why it matters.** atomik-kb is built so that nobody else can read your notes: not us, not
the person who runs your sync server, not a cloud provider. That also means nobody can
reset your access. If this computer is lost or broken, or you forget your app-lock
passphrase, the kit together with a backup file or your sync server is the only way back
in. Without it, your notes are gone for good.

**How to keep it safe.**

- Write the words down in order, or print them (**Print** in the kit screen). The app
  shows them only once.
- Keep the paper somewhere safe and offline. Do not store it in a note inside the app,
  and do not photograph it into a cloud gallery.
- Keep it in a different place from your backup files. Anyone who has both the kit and a
  backup can read everything.
- The app asks you to type three of the words back to prove you saved it correctly.
- If someone may have seen the kit, re-issue it in **Settings > Security > Recovery kit >
  Re-issue...**. The old kit stops working. Backups made earlier still need the old kit,
  so keep it with them or make a fresh backup afterwards.

You can choose **Do this later**. A banner then reminds you until the kit exists, and
backups and sharing are unavailable until you create it.

## App lock (optional)

**Settings > Security > App lock > Turn on app lock...** asks for a passphrase (at least 8
characters) when the app starts and after you lock it. You can also lock automatically
after 1, 5, 15 or 60 minutes of inactivity, and lock at any moment with **Lock now** in the
sidebar. If you forget the passphrase, only the recovery kit and a backup bring your notes
back.

## Your first page

Pick your space in the sidebar and choose **New page**. Type a title, press Enter and
start writing; type `/` for the block menu. Everything is saved as you type.
The next chapter, [Writing and organizing](writing-and-organizing.md), covers the rest.
