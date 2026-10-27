# Backup and restore

A **backup** is a single encrypted file (extension `.atomikkb`) holding your whole
workspace. Restoring it needs your **recovery kit**, so keep the two in different places.
You must create your recovery kit before you can make a backup. Sync is not a backup:
a deletion syncs too.

## Make a backup now

**Settings > Security > Backup > Export backup...** asks where to save the file and then shows
its size, creation time and how many documents it holds.

## Scheduled backups

In **Settings > Security**, **Scheduled backups** writes an encrypted backup automatically
while atomik-kb is running and unlocked. Choose:

- **Back up automatically** on or off, **Every day** or **Every week**.
- A **Folder** (**Choose folder...**), for example on an external drive or a synced
  folder, and/or **Google Drive**.
- **Backups to keep** (1 to 100): older scheduled backups in the folder and in Drive are
  deleted; no other file is touched.

**Back up now** runs one immediately, and the panel shows the result of the last run.

## Google Drive

Choose **Connect Google Drive** and finish signing in in your browser. atomik-kb asks only
for access to files it creates itself (a folder named **atomik-kb backups**), not the rest
of your Drive. Each backup is uploaded still encrypted: Google can see file names, sizes
and times and a small unencrypted header, but not what you wrote.

- **Open the folder in Drive** shows the files. **Disconnect** stops uploads and forgets
  the connection; files already in Drive stay until you delete them there.
- If Google stops accepting the saved sign-in, the panel asks you to **Reconnect**.
- If your build has no Google client built in, use **Advanced > Use your own Google
  client**: create a "Desktop app" OAuth client in the Google Cloud console with the Drive
  API enabled and the `drive.file` scope, then paste its client ID and secret. The secret
  is kept in the system credential store and is not part of backups.

## Restore

Restoring goes into a **new** installation (an existing workspace is never overwritten):

1. On the welcome screen choose **Restore from a backup**.
2. Choose the source: **A backup file...**, **A folder of backups...** (newest first) or
   **Google Drive** (sign in, then **Show my backups**).
3. Pick the backup and type your **recovery kit** (24 words; the first four letters of each
   word are enough, or use the code). If this installation already holds your account's key
   you are not asked.
4. The workspace is restored and the app restarts.

Afterwards you can reconnect sync in **Settings > Sync**.

## The recovery kit and backups

- A backup made before you re-issue your kit still needs the **old** kit. Keep the old kit
  with those backups, or make a new backup after re-issuing.
- The kit and the sync server are an alternative route: with the kit you can
  **Recover with my kit** from the server (see [Sync and devices](sync-and-devices.md)).
  Keep a backup file as well; it works even if the server is gone.

## Good practice

- Keep at least one backup off the computer (an external drive or Drive).
- Store the recovery kit on paper, elsewhere.
- Test a restore once on a spare machine: it is the only real proof that your backups work.
