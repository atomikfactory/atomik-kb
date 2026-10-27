// Develop and look at the UI in a plain browser, without Tauri or a real core:
//
//   pnpm --filter @atomik-kb/desktop dev      then open http://localhost:1420/dev.html
//
// The core is an in-memory fake (MemoryTransport) that forgets everything on reload.
// Query parameters pick the starting situation (see `dev-options.ts`):
//
//   ?seed=demo            a workspace with two spaces, nested pages and content (every
//                         block kind, links between pages, so backlinks and search show,
//                         an image and a file on "Welcome")
//   ?attachments=none     a transport without attachment bytes and file dialogs (the
//                         Image and File commands say why they are unavailable)
//   ?attachments=downloading  the demo (implied) whose image (2 chunks) and file
//                         (site-survey.pdf, 3 chunks) on "Welcome" came from another device:
//                         on the sync server, not here yet; they download one chunk every 1.5
//                         seconds ("Downloading n of m", then the picture and the file)
//   ?attachments=waiting  the same two, but the other device has not finished uploading them:
//                         "Waiting for another device", for good
//   ?attachments=on-demand  the same two under the "when opened" download policy: not
//                         downloaded until "Download" (or opening one) asks, then as above
//                         (these three connect sync to https://sync.example.com unless ?sync=
//                         says otherwise; Settings -> Sync shows the transfer counts)
//   ?shared=1             the demo's "Work" space is shared: this account (Maya) is its Owner, with Bob (Editor,
//                         verified), Carol (Commenter) and Dan (Viewer): the member panel (space menu, "Members and
//                         sharing..."), invitations by code and by link (each step of a session takes 1.5 seconds; the
//                         code is 7KQM X2TV R9HD and the digits 735 019, a code starting DEAD is dead), "Share this
//                         space..." on Personal; it adds the kit, sync and the demo seed. ?sharing= picks who this account is:
//   ?sharing=owner|editor|commenter|viewer   the role in the shared space (a Viewer's pages open read-only, the tree has
//                         no new page or rename, and every write the page tries answers Forbidden)
//   ?sharing=removed|closed   "Work" is a read-only copy (this account was removed from it, or an Owner closed it): the banner
//   ?sharing=disputed     an Owner sees an incomplete snapshot and Carol's refused changes
//   ?sharing=kit-notice   an Owner; a toast says that Bob's account added a device with its kit a moment after the page opens
//   ?comments=none|live   with a shared space ("Work"): it starts with comment threads on "Handbook" and "Weekly sync" (Carol and
//                         Bob, one anchored to the words "How we work.", one resolved, one on the page as a whole), a Person
//                         property "Owner" and two rows in the inbox (the bell, in the sidebar); none: without the threads
//                         and rows; live: and a few seconds after the page opens Bob replies in a thread and mentions you, then
//                         Carol starts a thread that mentions you (toasts, the bell's count). A Commenter ("?sharing=commenter")
//                         comments and cannot edit the page; a Viewer ("?sharing=viewer") reads the comments only
//   ?sharing=link-preview an invitation link was opened: the preview (Alice invites you to Project X as Editor, not verified)
//   ?sharing=code-invitee the dialog of joining by code is at the digits; ?sharing=code-inviter the Owner's code session
//   ?presence=demo        with a shared space (?shared=1, or ?sharing= for a role): its other members are on its pages, one
//                         step every 1.2 seconds. Bob edits "Handbook" (a caret with his name walks through its words, a word
//                         selected, then the caret at its end) and leaves for "Weekly sync" now and then; Carol reads "Handbook" and
//                         goes idle; Dan visits "Meetings" and "Handbook". Avatars (initials, the colour of their join order, the
//                         name as a tooltip) show in the page header and the space switcher
//   ?workspace=Research   onboarding already done (a workspace without spaces)
//   ?theme=dark           stored theme (light, dark or system)
//   ?language=fr          stored UI language (system, en, fr or es); without it the language follows the browser's
//   ?core=opening         core still starting; becomes ready after three seconds
//   ?core=failed          the OS keystore is unavailable (the fatal error screen)
//   ?api=2                core reports another API version (diagnostics warning)
//   ?encrypted=1          report an encrypted database, like the real desktop core
//   ?security=kit         the recovery kit exists (created now); with a workspace (the
//                         demo's name unless ?workspace or ?seed gives one)
//   ?security=lock        the kit, and the app lock on with the passphrase
//                         `correct horse battery` (Settings, "Lock now", auto-lock)
//   ?security=locked      the same, and the app starts locked (the lock screen); with
//                         ?core=opening it locks when the three seconds are over
//   ?sync=idle            connected to https://sync.example.com (a simulated server, no
//                         network), in a workspace: edits and "Sync now" run a fake cycle
//                         (Uploading -> Downloading -> Connected; the sidebar dot turns blue)
//   ?sync=offline         the same server, unreachable: Offline with its last error (amber dot)
//   ?sync=error           the same server, refusing: Error with its last error (red dot)
//   ?sync=forked          its history does not match this device's: the fork banner on Home
//                         and in Settings (red dot); "Disconnect" turns sync off
//   ?sync=invite          sync off, and the server does not know this account: connecting
//                         from Settings -> Sync ends in Error and asks for an invite token
//                         (the token `bad` is refused; any other one creates the account)
//
// Without ?sync, sync is off: Settings -> Sync connects to any http(s) address (the mirror
// accepts it, knows the account and starts healthy).
//
//   (each ?devices step of the fake pairing and recovery takes 2.5 seconds)
//   ?devices=pair-joiner  a fresh installation (no workspace, whatever else asks for one):
//                         "Add this device to my account", any http(s) address; the code
//                         ABCD EFGH JKMN shows, an imaginary device types it, the digits
//                         428 516 show; Confirm, and the account installs itself
//   ?devices=pair-sponsor a workspace with the kit, sync on and another device, opening on
//                         Settings: Security, Devices, "Add a device..." takes any well-formed
//                         code (BADB ADBA DBAD has expired); the digits 428 516, then "You
//                         are adding Pixel 9" once it confirmed; Confirm adds it
//   ?devices=recover      a fresh installation: "Recover with my kit", any http(s) address
//                         and the kit DEV_RECOVERY_KIT of dev-options.ts (23 times "abandon",
//                         then "art"; a kit with the word "wrong" is unknown to the server);
//                         then the reminder to re-issue the kit
//   ?devices=revoked      a workspace whose device was removed by another one: the removed
//                         screen, with "Export a backup"
//   ?devices=rotation     a workspace with the app lock on and three other devices, where
//                         another device rotated the keys: the banner asks for the passphrase
//                         `correct horse battery`; "Remove..." next to a device shows a new kit
//
// Without ?security the workspace has no kit yet, so Home shows the reminder to create
// one. Backups go to the in-memory core's own fake disk: "Export backup..." works, while
// "Restore from a backup" in the onboarding finds no file there (as after a cancelled
// dialog), since the page starts with an empty disk. The same fake disk serves the
// attachment dialogs: "Save as..." writes to `/downloads/`, and the Image and File commands
// pick the file written last (none at first: drop or paste a file instead). "Image from URL"
// (`/url`) has no network here: it knows one link, https://example.com/demo.png (the demo
// picture), and any other https link ends in the error card ("could not be reached").
//
// Import and export (ADR-0036) run on the same fake disk with reduced fidelity: an export is
// written to `/downloads/` (one page as a plain `.md` or `.html` file, anything else as a JSON
// listing standing in for the `.zip`), "Save as" of a CSV too, and the scheduled backups go to
// the folder `/backups/scheduled` (there is no timer here: "Back up now" writes one). The Import
// dialog has nothing to pick from on a page, so its three choices take canned sources
// (`demoImportSources`): ".zip" a small Notion export (a page with an id in its name, a sub-page,
// a database of two rows and a picture), "Folder" a folder of Markdown notes (front matter, a wiki
// link, a sub-folder), "Files" two loose files.
//
// Local AI and MCP (ADR-0037) run on fakes of the shell's commands:
//
//   ?ai=fake              AI and MCP are available (Settings -> AI), answered deterministically with a made-up
//                         list of models; the semantic index fills a few pages at a time; AI itself starts off
//   ?ai=fake&aiOn=1       the same, with AI already on (chat model qwen3:8b, embedding nomic-embed-text, r2t2
//                         for transcription): the writing assistant, Ctrl+J chat and Ctrl+K "Related" work
//   ?ai=ollama            the browser talks to a real Ollama at http://127.0.0.1:11434 (its default
//                         OLLAMA_ORIGINS allows this page); turn AI on in Settings -> AI to pick the models
//   ?ai=ollama&aiUrl=http://host:11434   another Ollama (a remote one needs its OLLAMA_ORIGINS to allow
//                         http://localhost:1420; the mirror's "consent dialog" accepts)
//   ?mcp=demo             with ?ai=: two MCP clients, one with read access, and a few audited calls
//
// Local dictation (ADR-0053) runs on a fake of the shell's commands, with the page's real microphone:
//
//   ?dictation=fake       `/microphone` and Settings -> Dictation are there: nothing installed or chosen, so the first
//                         use offers the download (ten steps of progress, then Verifying), then a scripted phrase comes
//                         in at the cursor for every two seconds of audio, and one more when you press Stop
//   ?dictation=fake&dictationInstalled=light   the same with Light (or accurate, or both: light,accurate) installed
//                         and chosen: `/microphone` starts at once
//
// Google Drive backups (ADR-0059) run on a fake of the shell's commands (`MemoryDrive`):
//
//   ?drive=fake           the Drive target (Settings -> Security -> Backup, with `&seed=demo` or `&workspace=`) and the
//                         wizard's Drive source (alone: onboarding, "Restore from a backup"): the consent is accepted, the
//                         sign-in takes a moment, the account is fake.user@example.com; the scheduled "Back up now"
//                         delivers to it
//   &driveConnected=1     starts connected          &driveConsent=no     refuses the consent dialog
//   &driveClient=none     a build with no Google client (the advanced section is the way in)
//   &driveFail=connect:denied,deliver:quota   fails operations (connect, list, download, deliver) with a reason of
//                         DRIVE_ERROR_REASONS (consentDeclined, cancelled, noClient, browserTimeout, denied, reconnect,
//                         quota, notConnected, notFound, network, server); `deliver:reconnect` shows the reconnect banner
//   &driveBackups=demo    three restorable scheduled backups in the Drive folder; the wizard asks for the recovery kit:
//                         absurd avoid scissors anxiety gather lottery category door army half long cage bachelor another
//                         expect people blade school educate curtain scrub monitor lady beyond
//   &driveBackups=3       three files that are not backups (restoring them fails)
//
// Only `index.html` is part of the production build; this file is not.

import "@atomik-kb/ui/styles.css";

import { CoreClient, MemoryTransport, demoImportSources } from "@atomik-kb/core-client";
import { App } from "@atomik-kb/ui";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import {
  DEV_SHARED_SPACE,
  commentsMode,
  initialRouteFromQuery,
  optionsFromQuery,
  seedMcpDemo,
  sharingMode,
  withDemoLinks,
  withoutAttachments,
} from "./dev-options";

const container = document.getElementById("root");
if (container === null) {
  throw new Error("dev.html is missing the #root element");
}

const query = new URLSearchParams(window.location.search);
const memory = new MemoryTransport({
  ...(await withDemoLinks(optionsFromQuery(query))),
  importSources: demoImportSources(),
});
if (query.get("mcp") === "demo" && memory.ai !== undefined) {
  await seedMcpDemo(memory);
}
if (query.get("sharing") === "kit-notice") {
  // A co-member's account adds a device with its recovery kit a moment after the page opens: the notice toast.
  const client = new CoreClient(memory);
  const work = (await client.spaceList()).find((space) => space.name === DEV_SHARED_SPACE);
  const bob = work === undefined ? undefined : (await client.spaceMembers(work.id)).members.find((entry) => entry.name === "Bob");
  if (work !== undefined && bob !== undefined) {
    setTimeout(() => {
      memory.receiveSharing({ type: "viaKit", spaceId: work.id, accountId: bob.accountId });
    }, 2500);
  }
}
if (commentsMode(query) === "live" && sharingMode(query) !== null) {
  // People of the shared space act a few seconds after the page opened: a reply that mentions this account, a new thread.
  const client = new CoreClient(memory);
  const work = (await client.spaceList()).find((space) => space.name === DEV_SHARED_SPACE);
  const handbook = work === undefined ? undefined : (await client.treeGet(work.id)).roots.find((node) => node.page.title === "Handbook");
  if (handbook !== undefined) {
    const pageId = handbook.page.id;
    setTimeout(() => {
      void client.commentList(pageId).then((threads) => {
        const open = threads.find((thread) => !thread.resolved);
        if (open !== undefined) {
          memory.receiveComments({
            type: "post",
            pageId,
            author: "Bob",
            threadId: open.threadId,
            body: [{ mention: "Maya" }, " here is a first draft of the rota."],
          });
        }
      });
    }, 6000);
    setTimeout(() => {
      memory.receiveComments({
        type: "post",
        pageId,
        author: "Carol",
        body: ["Does this need a review? ", { mention: "Maya" }],
      });
    }, 14000);
  }
}
const transport = query.get("attachments") === "none" ? withoutAttachments(memory) : memory;
if (query.get("core") === "opening") {
  setTimeout(() => {
    memory.setCoreState(query.get("security") === "locked" ? "locked" : "ready");
  }, 3000);
}

createRoot(container).render(
  <StrictMode>
    <App transport={transport} initialRoute={initialRouteFromQuery(query)} />
  </StrictMode>,
);
