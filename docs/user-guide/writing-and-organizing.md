# Writing and organizing

## Pages and blocks

A page is a title plus a sequence of **blocks**. Type `/` anywhere on a page to open the
block menu and keep typing to filter it. The menu offers:

- **Text:** text, bulleted list, numbered list, to-do list, toggle, quote, callout, code,
  divider, equation (and inline equation), table, table of contents, breadcrumb.
- **Links:** link to a page, mention a page, new sub-page, date, emoji. Pages that link to
  the current one are listed under **Linked from N pages** at the bottom (backlinks).
- **Media and files:** image, image from URL, photo gallery, PDF, video, audio, file,
  record audio. Paste or drop files and pictures straight onto a page. Images, files and
  recordings are encrypted like everything else.
- **Drawing:** diagram and whiteboard (changes are saved as you draw).
- **Databases:** list, table, board, calendar, gallery and dashboard views of your pages
  (see [Databases](databases.md)).
- **Reusable content:** template button (inserts a copy of its content above or below
  itself), new page from template.
- **AI and voice:** Ask AI, summarize page, action items, microphone. These work only
  when you have turned them on (see [AI and dictation](ai-and-dictation.md)).

Each page has an icon and a cover you can change, a font choice (default, serif or mono),
a small-text option, and a **Lock page** switch that makes it read-only until you unlock
it. The **Page options** menu also has **Copy as Markdown**, **Copy link to page**,
**Duplicate**, **Export...**, **Print...**, **Page history** and **Move to...**.

Undo and redo (`Ctrl+Z`, `Ctrl+Shift+Z` or `Ctrl+Y`; `Cmd` on macOS) work in the editor and
also undo structural changes such as moving or trashing a page. Use `Ctrl+F` to find (and
replace) inside the current page.

## Spaces and the page tree

A **space** is a top-level container, for example *Personal* and *Work*. Each space has its
own page tree, its own properties and tags, and can be shared on its own (see
[Sharing](sharing.md)).

- Switch space or create one with **New space** at the top of the sidebar.
- The space's **options** menu lets you rename it, change its icon, reorder it, sort its
  pages A-Z or by date created, and delete it. Deleting a space deletes its pages,
  including the trash, permanently; you must type the space's name to confirm.
- Pages nest to any depth. Drag a page in the sidebar to move it or re-parent it, and drop
  a page from the sidebar onto another page to insert a mention.
- **Move to...** (in a page's menu) moves a page, with its sub-pages, links and history,
  to another space. The dialog tells you before you confirm what will change, for example
  which properties are added to the target space and which relation links are dropped.

## Tabs

Open pages appear in tabs.

- Clicking a page replaces the page in the current tab; **Open in new tab** keeps both.
  A page can be open in only one tab.
- `Ctrl+T` opens a new tab, `Ctrl+W` closes one. Right-click or use the tab menu for
  **Close other tabs**, **Close tabs to the right**, **Reopen closed tab** and **Pin tab**.
  Pinned tabs stay put.
- Each tab has its own history: **Back** and **Forward** (`Alt+Left` and `Alt+Right`).
- `Ctrl+\` hides or shows the sidebar.

## Favourites and recent pages

Choose **Add to favourites** in a page's menu to pin it to the **Favourites** list in the
sidebar; use **Move up** and **Move down** to order them. **Recent pages** appear in the
search and on **Home**. The recent list lives only on this device and is never synced;
clear it in **Settings**.

## Tags and properties

Every space has a built-in **Tags** property. Add tags from a page's properties and
click a tag to find the pages that carry it. Beyond tags, you can define your own
typed properties for the whole space; see [Databases](databases.md).

## Search

`Ctrl+K` (or the **Search** button in the sidebar) searches titles and content in every
space. Narrow it with filters: **Tag**, **Space** (all spaces or this one), **Edited**
(today, past 7 days, past 30 days) and **Titles only**. The index lives inside your
encrypted database; it is never synced. If you turn on the optional semantic index (see
[AI and dictation](ai-and-dictation.md)), the search also finds pages related by meaning.

## Templates

A **template** is a page that new pages are copied from. Use **Turn into template** in a
page's menu (and **Stop using as template** to undo it). Templates are not shown in the
sidebar; open **Templates** or use **New page from template...** (also in the `/` menu) to
pick one. Some starter templates are included.

Template pages can hold placeholders that are filled in when a page is created: **Today**,
**Now** and **Me** (whoever creates the page). Databases can also have their own
templates, with a default one, under the **New page** options of a view.

## Trash

Deleting a page moves it, with its sub-pages, to the **Trash** in the sidebar. From there
you can **Restore** or **Delete permanently**. By default pages are removed for good after
30 days; change it in **Settings > Trash** (0 keeps pages until you delete them yourself).
When you sync, a deletion reaches your other devices, so the shortest limit among them
applies. Files that no page uses any more are cleaned up after the period set in
**Settings > Attachments** (**Keep files no page uses for**).

## Page history

**Page options > Page history** lists the versions of a page: dated editing sessions, newest
first, and older undated ones. Select a version to preview it; **Show changes** highlights
what was added, removed or changed compared with the page now. **Restore this version**
turns the page (content and title) back into that state. This is itself a new change, so
the version you had before stays in the list and you can undo the restore.

The list of dated sessions is kept on each device and is not synced or backed up; older
versions are still reachable as undated entries. You can view the history of
a page you cannot edit, but only editors can restore.

## Reminders and dates

A date mention can carry a time and a **Reminder** (at the time, 5, 15, 30 or 60 minutes
before, or at 9:00). Reminders fire while the app is running; turn on **Settings > Desktop >
Keep running in the tray when the window is closed** to keep them coming.

## Desktop conveniences

- **Quick capture:** the global shortcut (default `Ctrl+Alt+N`,
  configurable in **Settings > Desktop**) opens atomik-kb with a new page from any program.
- The menu bar (File, Edit, View, Window, Help) is hidden until you press `Alt`.
- Links of the form `atomik-kb://...` open the right page.
- **Settings > Appearance** has the theme (light, dark, match system), accent colour and
  language.
