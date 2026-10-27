# Import and export

You can bring notes in from other apps and take your pages out in open formats. Everything
is read and written on your own computer.

## Export

**Page options > Export...** (or **File > Export...**; press `Alt` to show the menu bar)
exports the current page. A space's options menu has **Export space...**, which saves every
page of the space into one `.zip` file.

| Format | What you get |
|---|---|
| **Markdown** | Plain text with front matter. Pictures and files travel in the same `.zip`, and atomik-kb can import it back. |
| **HTML** | A web page you can open in any browser. |
| **PDF** | Opens the print dialog; choose "Save as PDF" there. |

**Include sub-pages** adds the page's descendants. If something cannot be represented in
the chosen format, the confirmation says how many items were left out. Other ways out:

- **Copy as Markdown** in the page options menu.
- **Print...** (`Ctrl+P`) for a printed or PDF copy.
- **Export CSV** from a database view's **View actions**.
- A [backup](backup-and-restore.md) holds everything, but only atomik-kb can read it.

## Import

**File > Import...**, or **Import into this page...** in a page's menu (the imported pages
become sub-pages of that page), or **Import...** in a space's menu (they are added at the
top level of the space). Choose what to import:

- **Files:** Markdown, text, HTML or CSV files.
- **A folder:** a folder and everything in it.
- **A .zip file:** a Notion "Markdown & CSV" export, an AppFlowy backup or an atomik-kb
  export.

CSV files become pages with properties for their columns. Folders and zip files keep their
structure as sub-pages, and links between the imported files are rewired. Attached
images and files come along as attachments.

When it finishes, a report lists the pages, attachments and properties created, and
groups anything that was **left out or changed**, for example files that were skipped, links
to something not in the import, images on the web (kept as links, not downloaded),
unsupported content, or property values that did not fit. Nothing is fetched from the
internet without asking you first. Imports are limited in size, and the report says if
pages were left out for being too many.

## Import an AppFlowy workspace

In AppFlowy, open **Settings > Manage data > Back up your workspace** and save the
`.zip`. In atomik-kb, choose **Import AppFlowy workspace...** (in a space's options menu) and pick that file. Each AppFlowy space becomes a **space of its own** next to
the ones you have; nothing you have now is changed. The result lists the new spaces with
an **Open** button for each.

## Moving pages between spaces

This is not an import, but often what you want: **Move to...** in a page's menu moves a
page with its sub-pages, links and history into another space. See
[Writing and organizing](writing-and-organizing.md).
