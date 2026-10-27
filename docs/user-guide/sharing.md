# Sharing

atomik-kb lets you share a whole **space** with other people, and publish a single page as
a read-only **view link**. Sharing is end-to-end encrypted: your sync server learns who
belongs to a space, never what is in it.

## Before you share

- Turn on sync (**Settings > Sync**): people join through your sync server.
- Create your recovery kit (**Settings > Security**): it gives you the identity other people
  see.
- Everyone you invite receives the space's **whole history**, including pages in the trash
  and text deleted long ago. Members keep their copies; removing a member cannot take
  them back. The share dialog counts what would be sent before you confirm.

## Share a space

Open the space's options menu and choose **Share this space...**, enter the name members
will see next to your changes, and choose **Share**. The space becomes a shared space (the
sidebar shows a **Shared space** badge with the member count). Then use **Invite people...**
or **Members and sharing...** from the same menu.

## Roles

| Role | Can |
|---|---|
| **Owner** | Everything: members and invitations, space settings, closing the space, deleting pages for good |
| **Editor** | Edit pages, structure and files; cannot delete pages for good |
| **Commenter** | Read pages and comment on them; cannot change content |
| **Viewer** | Read pages only |

A space always keeps at least one Owner. Change a member's role in **Members and sharing...**.
People with a read-only role see a **Read only** marker and the editor is locked.

## Invite someone

**Invite with a code...** (the safest):

1. Give the person the code shown and your server address. The code works for 10 minutes.
2. They choose **Join a shared space...** and type both, and you both see **six digits**.
3. Confirm only if the digits match. The person joins with the role you chose, and you
   see each other as **Verified**.

**Invite with a link...** creates a link that the first person to open it uses to join,
once, with the role you pick, for the number of days you choose. The link is shown only
once. They are *not* verified until you compare safety numbers, so send it only to the
person you mean. Open invitations can be revoked in the Invitations list.

## Verify people with safety numbers

Each member has a **safety number** of 30 digits. Compare it with the person in person or
on a call you trust. If it matches, mark them **Verified**. This is how you notice if
someone is impersonating a person.

## Working together

- **Comments:** select text and choose **Comment**, or comment on the page, and use
  **@** to mention a person. Threads can be resolved and shown again with **Show resolved**.
- **Mentions and the Person property:** mention people in pages, assign them in
  **Person** properties, and filter with "is me".
- **Presence:** you see who else is on the page, in real time while devices are online.
- **Notices:** the app tells you when your role changes, when you are removed, or when
  a space is closed. Changes that a member's role does not allow are refused and reported.

## Space settings

**Members and sharing...** includes **Space settings**: how long trashed pages are kept
(only Owners' devices delete from the trash), and **Editors may create view links**.

## Leaving, removal and closing

- **Leave this space:** you lose access and the local copy is deleted. Export it first if you
  want to keep a copy. You can be invited again.
- **Remove a member** (Owners): they lose access to everything written from now on and
  everyone else gets a new key. They keep what they already received.
- **Close this space** (Owners): nobody can change it any more. Every member keeps a
  read-only copy and the server deletes its own copy after 30 days. This cannot be undone.
- A removed or closed space stays on your device as a **read-only copy** until you remove it.

## Local AI in a shared space

AI is off in a shared space until each member turns it on for themselves
(**Use AI in this space**). Then the AI on that member's device may read what they can read.

## View-only links

A view link lets anyone read **one page** in a web browser, with no account.

Create one from the page's menu with **Share a view link...**, choose an expiry (never or a
number of days) and **Create the link**. The link is shown only once: copy it. List and
**Revoke** the links of a page from the same dialog; the person who made a link or an
Owner can revoke it.

What a link shows: the page as it reads now, without its history (text you deleted is not in
it), without comments, without sub-pages and without the rest of the space. While a
device of the space is online, the page follows your edits within about a minute.

Know the trade-offs:

- Anyone who holds the link can read the page. Treat it like a password.
- The page is kept on your sync server, which also serves the viewer, so the operator of
  that server can read this one page. For a server you do not run, that matters.
- The decryption key sits after the `#` in the link, which browsers never send to the
  server; if the page is reloaded without the full link it asks for it again.
- By default only Owners can create view links; an Owner can allow Editors.
- Your server must be configured to serve the viewer (`KB_SERVER_VIEWER_DIR` pointing at a
  build of `apps/viewer`). The stock Docker image does not include the viewer, so view links
  need a server set up for it.
