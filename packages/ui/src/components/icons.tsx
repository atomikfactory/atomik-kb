// Small inline icons that follow the text color. All are decorative: the control that
// holds one carries the accessible name.

import type { SVGProps } from "react";

function Svg(props: SVGProps<SVGSVGElement>) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...props}
    />
  );
}

export function ChevronIcon() {
  return (
    <Svg>
      <path d="M6 3.5 10.5 8 6 12.5" />
    </Svg>
  );
}

export function ChevronDownIcon() {
  return (
    <Svg>
      <path d="m3.5 6 4.5 4.5L12.5 6" />
    </Svg>
  );
}

export function PlusIcon() {
  return (
    <Svg>
      <path d="M8 3v10M3 8h10" />
    </Svg>
  );
}

export function MoreIcon() {
  return (
    <Svg fill="currentColor" stroke="none">
      <circle cx="3.5" cy="8" r="1.3" />
      <circle cx="8" cy="8" r="1.3" />
      <circle cx="12.5" cy="8" r="1.3" />
    </Svg>
  );
}

/** A clock with a counter-clockwise arrow: the page's history. */
export function HistoryIcon() {
  return (
    <Svg>
      <path d="M2.5 8a5.5 5.5 0 1 0 1.7-4" />
      <path d="M2.5 2.5v3h3" />
      <path d="M8 5v3.2l2 1.3" />
    </Svg>
  );
}

export function UndoIcon() {
  return (
    <Svg>
      <path d="M5.5 2.5 2.5 5.5l3 3" />
      <path d="M2.5 5.5h6.25a3.75 3.75 0 0 1 0 7.5H5" />
    </Svg>
  );
}

export function RedoIcon() {
  return (
    <Svg>
      <path d="m10.5 2.5 3 3-3 3" />
      <path d="M13.5 5.5H7.25a3.75 3.75 0 0 0 0 7.5H11" />
    </Svg>
  );
}

export function TrashIcon() {
  return (
    <Svg>
      <path d="M2.5 4.5h11M6 4.5V3h4v1.5M4 4.5l.6 8.5h6.8l.6-8.5M6.75 7v3.75M9.25 7v3.75" />
    </Svg>
  );
}

/** The icon of a page that has none of its own. */
export function PageGlyph() {
  return (
    <Svg>
      <path d="M4 2h5.25L12 4.75V14H4z" />
      <path d="M9 2v3h3" />
    </Svg>
  );
}

/** The icon of a space that has none of its own. */
export function SpaceGlyph() {
  return (
    <Svg>
      <rect x="2.5" y="2.5" width="11" height="11" rx="3" />
    </Svg>
  );
}

/** The grip of the block handle (drag, or click for the block menu). */
export function GripIcon() {
  return (
    <Svg fill="currentColor" stroke="none">
      <circle cx="5.5" cy="3.5" r="1.2" />
      <circle cx="10.5" cy="3.5" r="1.2" />
      <circle cx="5.5" cy="8" r="1.2" />
      <circle cx="10.5" cy="8" r="1.2" />
      <circle cx="5.5" cy="12.5" r="1.2" />
      <circle cx="10.5" cy="12.5" r="1.2" />
    </Svg>
  );
}

/** Search (the sidebar's search button). */
export function SearchIcon() {
  return (
    <Svg>
      <circle cx="7" cy="7" r="4.25" />
      <path d="m10.25 10.25 3.25 3.25" />
    </Svg>
  );
}

/** A padlock (the lock screen, "Lock now"). */
export function LockIcon() {
  return (
    <Svg>
      <rect x="3.25" y="7" width="9.5" height="6.75" rx="1.5" />
      <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" />
    </Svg>
  );
}

/** The sidebar: a framed window with its left column (hide or show the sidebar). */
export function SidebarIcon() {
  return (
    <Svg>
      <rect x="2" y="3" width="12" height="10" rx="2" />
      <path d="M6.25 3v10" />
    </Svg>
  );
}

/** A star, outline: a page that is not a favourite. */
export function StarIcon() {
  return (
    <Svg>
      <path d="m8 2.25 1.75 3.55 3.9.55-2.82 2.75.67 3.9L8 11.15l-3.5 1.85.67-3.9L2.35 6.35l3.9-.55z" />
    </Svg>
  );
}

/** A star, filled: a favourite. */
export function StarFilledIcon() {
  return (
    <Svg fill="currentColor">
      <path d="m8 2.25 1.75 3.55 3.9.55-2.82 2.75.67 3.9L8 11.15l-3.5 1.85.67-3.9L2.35 6.35l3.9-.55z" />
    </Svg>
  );
}

// ----- Block glyphs of the `/` menu (editor/slash-commands.ts, one per entry) -----

/** Plain text: a capital T. */
export function TextIcon() {
  return (
    <Svg>
      <path d="M3.5 4.75V3h9v1.75M8 3v10M6 13h4" />
    </Svg>
  );
}

/** Heading 1: an H and a 1. */
export function Heading1Icon() {
  return (
    <Svg>
      <path d="M2 3.5v9M2 8h5.5M7.5 3.5v9" />
      <path d="m10.25 6.5 1.75-1.25v7.25" />
    </Svg>
  );
}

/** Heading 2: an H and a 2. */
export function Heading2Icon() {
  return (
    <Svg>
      <path d="M2 3.5v9M2 8h5.5M7.5 3.5v9" />
      <path d="M10 7a1.75 1.75 0 0 1 3.5 0c0 1.25-1.5 2.25-3.5 5.5h3.75" />
    </Svg>
  );
}

/** Heading 3: an H and a 3. */
export function Heading3Icon() {
  return (
    <Svg>
      <path d="M2 3.5v9M2 8h5.5M7.5 3.5v9" />
      <path d="M10 6.25Q10.25 5 11.75 5Q13.5 5 13.5 6.4Q13.5 7.8 11.75 7.9Q13.75 8.1 13.75 9.8Q13.75 12.75 11.75 12.75Q10.25 12.75 10 11.5" />
    </Svg>
  );
}

/** Bulleted list: dots and lines. */
export function BulletListIcon() {
  return (
    <Svg>
      <circle cx="3" cy="4" r="0.9" fill="currentColor" stroke="none" />
      <circle cx="3" cy="8" r="0.9" fill="currentColor" stroke="none" />
      <circle cx="3" cy="12" r="0.9" fill="currentColor" stroke="none" />
      <path d="M6.5 4h7M6.5 8h7M6.5 12h7" />
    </Svg>
  );
}

/** Numbered list: 1, 2, 3 and lines. */
export function NumberedListIcon() {
  return (
    <Svg strokeWidth="1.25">
      <path d="M2.2 3.4 3.3 2.5v3.3" />
      <path d="M2.2 7.3Q2.4 6.2 3.3 6.2Q4.3 6.3 4.2 7.2Q4.1 7.9 2.2 9.5H4.3" />
      <path d="M2.2 11Q2.5 10.3 3.3 10.3Q4.2 10.4 4.2 11.1Q4.2 11.8 3.2 11.9Q4.4 12 4.4 12.8Q4.3 13.7 3.3 13.7Q2.5 13.7 2.2 13" />
      <path d="M7 4h7M7 8h7M7 12h7" strokeWidth="1.5" />
    </Svg>
  );
}

/** To-do list: a ticked box. */
export function CheckIcon() {
  return (
    <Svg>
      <rect x="2.5" y="2.5" width="11" height="11" rx="2.5" />
      <path d="m5.5 8.25 1.75 1.75 3.25-3.5" />
    </Svg>
  );
}

/** Toggle: a closed disclosure triangle and the text it hides. */
export function ToggleIcon() {
  return (
    <Svg>
      <path d="M3 4.5 7 8l-4 3.5z" fill="currentColor" strokeWidth="1" />
      <path d="M9.5 5.5h4M9.5 10.5h4" />
    </Svg>
  );
}

/** Toggle heading 1: the disclosure triangle, an H and a 1. */
export function ToggleHeading1Icon() {
  return (
    <Svg>
      <path d="M1.5 5 4.5 8l-3 3z" fill="currentColor" strokeWidth="1" />
      <path d="M6.5 3.5v9M6.5 8H10.5M10.5 3.5v9" />
      <path d="m12.25 6.5 1.5-1.25v7.25" />
    </Svg>
  );
}

/** Toggle heading 2: the disclosure triangle, an H and a 2. */
export function ToggleHeading2Icon() {
  return (
    <Svg>
      <path d="M1.5 5 4.5 8l-3 3z" fill="currentColor" strokeWidth="1" />
      <path d="M6.5 3.5v9M6.5 8H10.5M10.5 3.5v9" />
      <path d="M12 6.75a1.4 1.4 0 0 1 2.8 0c0 1-1.2 1.8-2.8 4.5h3" />
    </Svg>
  );
}

/** Toggle heading 3: the disclosure triangle, an H and a 3. */
export function ToggleHeading3Icon() {
  return (
    <Svg>
      <path d="M1.5 5 4.5 8l-3 3z" fill="currentColor" strokeWidth="1" />
      <path d="M6.5 3.5v9M6.5 8H10.5M10.5 3.5v9" />
      <path d="M12 6Q12.2 5 13.4 5Q14.8 5 14.8 6.1Q14.8 7.2 13.4 7.3Q15 7.5 15 8.9Q15 11.5 13.4 11.5Q12.2 11.5 12 10.5" />
    </Svg>
  );
}

/** Quote: a bar beside lines of text. */
export function QuoteIcon() {
  return (
    <Svg>
      <path d="M3.25 3v10" strokeWidth="2.25" />
      <path d="M7 5h6.5M7 8h6.5M7 11h4" />
    </Svg>
  );
}

/** Callout: a framed note. */
export function CalloutIcon() {
  return (
    <Svg>
      <rect x="1.75" y="3" width="12.5" height="10" rx="2.25" />
      <circle cx="5.25" cy="8" r="1" fill="currentColor" stroke="none" />
      <path d="M8 8h3.5" />
    </Svg>
  );
}

/** Template button: a button with a plus over the lines of the content it repeats. */
export function TemplateButtonIcon() {
  return (
    <Svg>
      <rect x="2" y="2" width="12" height="5" rx="1.75" />
      <path d="M8 3.6v1.8M7.1 4.5h1.8" />
      <path d="M3 10.2h10M3 13h6.5" />
    </Svg>
  );
}

/** Divider: a rule between two blocks. */
export function DividerIcon() {
  return (
    <Svg>
      <path d="M2 8h12" />
      <path d="M5 4.5h6M5 11.5h6" opacity="0.45" />
    </Svg>
  );
}

/** Code: angle brackets and a slash. */
export function CodeIcon() {
  return (
    <Svg>
      <path d="m5.25 4.5-3.5 3.5 3.5 3.5M10.75 4.5l3.5 3.5-3.5 3.5M9 3l-2 10" />
    </Svg>
  );
}

/** Diagram: two boxes and the arrow between them. */
export function DiagramIcon() {
  return (
    <Svg>
      <rect x="2" y="2.5" width="5.5" height="3.75" rx="1" />
      <rect x="8.5" y="9.75" width="5.5" height="3.75" rx="1" />
      <path d="M4.75 6.25v3.25a1 1 0 0 0 1 1h2.75" />
    </Svg>
  );
}

/** Equation: a sigma. */
export function EquationIcon() {
  return (
    <Svg>
      <path d="M12 3.5H4.5L8.5 8l-4 4.5H12" />
    </Svg>
  );
}

/** Inline equation: a small sigma between two stubs of text on its line. */
export function InlineEquationIcon() {
  return (
    <Svg>
      <path d="M10.5 4.75H6.25L9 8l-2.75 3.25h4.25" />
      <path d="M1.5 8h1.5M13 8h1.5" />
    </Svg>
  );
}

/** Table: a grid. */
export function TableIcon() {
  return (
    <Svg>
      <rect x="2" y="2.5" width="12" height="11" rx="1.75" />
      <path d="M2 6.25h12M2 9.75h12M6 2.5v11M10 2.5v11" />
    </Svg>
  );
}

/** Two columns: two side-by-side panels. */
export function Columns2Icon() {
  return (
    <Svg>
      <rect x="2" y="2.5" width="5" height="11" rx="1.25" />
      <rect x="9" y="2.5" width="5" height="11" rx="1.25" />
    </Svg>
  );
}

/** Three columns: three side-by-side panels. */
export function Columns3Icon() {
  return (
    <Svg>
      <rect x="2" y="2.5" width="3.2" height="11" rx="1" />
      <rect x="6.4" y="2.5" width="3.2" height="11" rx="1" />
      <rect x="10.8" y="2.5" width="3.2" height="11" rx="1" />
    </Svg>
  );
}

/** Table of contents: a heading and indented entries. */
export function TocIcon() {
  return (
    <Svg>
      <path d="M2 4h12M5.5 8H14M5.5 12h6" />
      <path d="M2.5 8h.01M2.5 12h.01" />
    </Svg>
  );
}

/** Breadcrumb: path segments separated by chevrons. */
export function BreadcrumbIcon() {
  return (
    <Svg>
      <path d="M1.5 8h2M9 8h1.5" />
      <path d="m5.5 6 2 2-2 2M12 6l2 2-2 2" />
    </Svg>
  );
}

/** List view: a framed list of rows (a saved view over pages). */
export function ListViewIcon() {
  return (
    <Svg>
      <rect x="2" y="2.5" width="12" height="11" rx="1.75" />
      <path d="M2 6.2h12M2 9.8h12" />
      <path d="M4.5 4.35h4M4.5 8h4M4.5 11.65h4" />
    </Svg>
  );
}

/** Table view: a framed grid with a header row (a database table over pages). */
export function TableViewIcon() {
  return (
    <Svg>
      <rect x="2" y="2.5" width="12" height="11" rx="1.75" />
      <path d="M2 6h12M2 9.75h12M6.5 6v7.5" />
      <path d="M4 4.25h8" />
    </Svg>
  );
}

/** Board view: three columns of cards (a kanban board over pages). */
export function BoardViewIcon() {
  return (
    <Svg>
      <rect x="2" y="2.5" width="3.4" height="7" rx="1" />
      <rect x="6.3" y="2.5" width="3.4" height="11" rx="1" />
      <rect x="10.6" y="2.5" width="3.4" height="5" rx="1" />
    </Svg>
  );
}

/** Calendar view: a framed month grid under a header band (a calendar of pages by date). */
export function CalendarViewIcon() {
  return (
    <Svg>
      <rect x="2" y="2.5" width="12" height="11" rx="1.75" />
      <path d="M2 6h12M5.5 6v7.5M9 6v7.5M12.5 6v7.5M2 9.75h12" />
      <path d="M5 1.5v2M11 1.5v2" />
    </Svg>
  );
}

/** Gallery view: two cards with a picture above their title lines (a gallery of pages). */
export function GalleryViewIcon() {
  return (
    <Svg>
      <rect x="2" y="2.25" width="5.25" height="11.5" rx="1.25" />
      <rect x="8.75" y="2.25" width="5.25" height="11.5" rx="1.25" />
      <path d="M2 7h5.25M8.75 7H14M3.5 10h2.25M10.25 10h2.25" />
    </Svg>
  );
}

/** Feed view: two wide cards one under the other, each with its title and a line of text (ADR-0058). */
export function FeedViewIcon() {
  return (
    <Svg>
      <rect x="2" y="2" width="12" height="5.25" rx="1.25" />
      <rect x="2" y="8.75" width="12" height="5.25" rx="1.25" />
      <path d="M4.5 4.6h7M4.5 11.35h7" />
    </Svg>
  );
}

/** Dashboard: a large tile, a small one and a bar chart (ADR-0055). */
export function DashboardViewIcon() {
  return (
    <Svg>
      <rect x="2" y="2.25" width="12" height="5" rx="1.25" />
      <rect x="2" y="9.25" width="4.5" height="4.5" rx="1.25" />
      <path d="M9 13.75v-2.5M11.5 13.75v-4M14 13.75V10" />
    </Svg>
  );
}

/** Image: a framed picture with a hill and a sun. */
export function ImageIcon() {
  return (
    <Svg>
      <rect x="2" y="2.75" width="12" height="10.5" rx="1.75" />
      <circle cx="5.75" cy="6.5" r="1" />
      <path d="m2.5 12 3.75-3.5 2.75 2.5 2-1.75 2.5 2.25" />
    </Svg>
  );
}

/** Photo gallery: four small frames in a grid. */
export function GalleryIcon() {
  return (
    <Svg>
      <rect x="2.25" y="2.25" width="5" height="5" rx="1" />
      <rect x="8.75" y="2.25" width="5" height="5" rx="1" />
      <rect x="2.25" y="8.75" width="5" height="5" rx="1" />
      <rect x="8.75" y="8.75" width="5" height="5" rx="1" />
    </Svg>
  );
}

/** PDF: a page with a folded corner and two lines of text. */
export function PdfIcon() {
  return (
    <Svg>
      <path d="M4 1.75h5.25L12.5 5v8.25a1 1 0 0 1-1 1h-7.5a1 1 0 0 1-1-1v-10.5a1 1 0 0 1 1-1Z" />
      <path d="M9 1.75V5h3.5M5.5 8.25h5M5.5 10.75h5" />
    </Svg>
  );
}

/** Video: a frame with a play triangle. */
export function VideoIcon() {
  return (
    <Svg>
      <rect x="2" y="3" width="12" height="10" rx="1.5" />
      <path d="M6.75 6v4l3.25-2Z" />
    </Svg>
  );
}

/** Audio: a speaker with sound waves. */
export function AudioIcon() {
  return (
    <Svg>
      <path d="M2.5 6.25h2.25L8 3.5v9L4.75 9.75H2.5Z" />
      <path d="M10.25 6a2.9 2.9 0 0 1 0 4M12 4.5a5 5 0 0 1 0 7" />
    </Svg>
  );
}

/** Zoom out: a minus. */
export function MinusIcon() {
  return (
    <Svg>
      <path d="M3.5 8h9" />
    </Svg>
  );
}

/** Fit to width: two arrows pointing at the sides. */
export function FitWidthIcon() {
  return (
    <Svg>
      <path d="M2 3v10M14 3v10M4.5 8h7M6.5 6 4.5 8l2 2M9.5 6l2 2-2 2" />
    </Svg>
  );
}

/** File: a paperclip. */
export function FileIcon() {
  return (
    <Svg>
      <path d="m12.5 7.25-4.6 4.6a2.6 2.6 0 0 1-3.7-3.7l4.6-4.6a1.75 1.75 0 0 1 2.5 2.5L6.7 10.65a.9.9 0 0 1-1.3-1.3L9.75 5" />
    </Svg>
  );
}

/** Link: two chain links. */
export function LinkIcon() {
  return (
    <Svg>
      <rect x="2.25" y="9" width="6" height="3.5" rx="1.75" transform="rotate(-45 5.25 10.75)" />
      <rect x="7.75" y="3.5" width="6" height="3.5" rx="1.75" transform="rotate(-45 10.75 5.25)" />
      <path d="M6.9 9.1 9.1 6.9" />
    </Svg>
  );
}

/** A new page inside this one: a page with a plus. */
export function SubPageIcon() {
  return (
    <Svg>
      <path d="M4 2h5.25L12 4.75V14H4z" />
      <path d="M9 2v3h3" />
      <path d="M8 8v4M6 10h4" />
    </Svg>
  );
}

/** Emoji: a smiling face. */
export function EmojiIcon() {
  return (
    <Svg>
      <circle cx="8" cy="8" r="6.25" />
      <path d="M5.5 9.5a3 3 0 0 0 5 0" />
      <circle cx="5.9" cy="6.4" r="0.8" fill="currentColor" stroke="none" />
      <circle cx="10.1" cy="6.4" r="0.8" fill="currentColor" stroke="none" />
    </Svg>
  );
}

/** Date: a calendar page with its two rings. */
export function CalendarIcon() {
  return (
    <Svg>
      <rect x="2.25" y="3.25" width="11.5" height="10.5" rx="1.75" />
      <path d="M2.25 6.75h11.5M5.5 1.75v3M10.5 1.75v3" />
    </Svg>
  );
}

/** Reminder: a bell. */
export function BellIcon() {
  return (
    <Svg>
      <path d="M3.75 11.25h8.5l-1.1-1.6V6.9a3.15 3.15 0 0 0-6.3 0v2.75zM6.6 13.4a1.5 1.5 0 0 0 2.8 0" />
    </Svg>
  );
}

/** Mention: an at sign. */
export function AtIcon() {
  return (
    <Svg>
      <circle cx="8" cy="8" r="2.5" />
      <path d="M10.5 8v1a1.75 1.75 0 0 0 3.5 0V8a6 6 0 1 0-2.3 4.7" />
    </Svg>
  );
}

/** Align left: four lines, flush with the left edge. */
export function AlignLeftIcon() {
  return (
    <Svg>
      <path d="M2.5 3.5h11M2.5 6.5h7M2.5 9.5h11M2.5 12.5h7" />
    </Svg>
  );
}

/** Align center: four lines, centered. */
export function AlignCenterIcon() {
  return (
    <Svg>
      <path d="M2.5 3.5h11M4.5 6.5h7M2.5 9.5h11M4.5 12.5h7" />
    </Svg>
  );
}

/** Align right: four lines, flush with the right edge. */
export function AlignRightIcon() {
  return (
    <Svg>
      <path d="M2.5 3.5h11M6.5 6.5h7M2.5 9.5h11M6.5 12.5h7" />
    </Svg>
  );
}

/** Caption: a picture frame with a line of text under it. */
export function CaptionIcon() {
  return (
    <Svg>
      <rect x="2.25" y="2.25" width="11.5" height="7" rx="1.5" />
      <path d="M4.5 12.5h7" />
    </Svg>
  );
}

/** View full size: arrows pointing out of the corners. */
export function ExpandIcon() {
  return (
    <Svg>
      <path d="M9.5 2.5h4v4M13.5 2.5 9 7M6.5 13.5h-4v-4M2.5 13.5 7 9" />
    </Svg>
  );
}

/** Whiteboard: a board with a pencil stroke. */
export function WhiteboardIcon() {
  return (
    <Svg>
      <rect x="1.75" y="2.75" width="12.5" height="9.5" rx="1.5" />
      <path d="M4.5 9.5c1.2-3 2.3-3 3-1.2.6 1.4 1.8 1.2 3.5-2" />
      <path d="M6 14.25h4" />
    </Svg>
  );
}

/** Back in the tab's history (an arrow to the left). */
export function BackIcon() {
  return (
    <Svg>
      <path d="M13 8H3.5M7.5 3.75 3.25 8l4.25 4.25" />
    </Svg>
  );
}

/** Forward in the tab's history (an arrow to the right). */
export function ForwardIcon() {
  return (
    <Svg>
      <path d="M3 8h9.5M8.5 3.75 12.75 8 8.5 12.25" />
    </Svg>
  );
}

/** Closes a tab (a small cross). */
export function CloseIcon() {
  return (
    <Svg width="12" height="12" viewBox="0 0 12 12">
      <path d="m3 3 6 6M9 3 3 9" />
    </Svg>
  );
}

/** The Home of a space: the tab that shows the workspace name and the recent pages. */
export function HomeIcon() {
  return (
    <Svg>
      <path d="M2.5 7.25 8 2.5l5.5 4.75M4 6.5v6.25h8V6.5M6.75 12.75v-3.5h2.5v3.5" />
    </Svg>
  );
}

/** Two people: a shared space (the space switcher shows it with the member count). */
export function UsersIcon() {
  return (
    <Svg>
      <circle cx="6" cy="5.5" r="2.25" />
      <path d="M1.75 13c.25-2.25 2.1-3.5 4.25-3.5s4 1.25 4.25 3.5" />
      <path d="M10.5 3.5a2.25 2.25 0 0 1 0 4.25M11.5 9.75c1.6.4 2.6 1.5 2.75 3.25" />
    </Svg>
  );
}

/** Comments: a speech bubble. */
export function CommentIcon() {
  return (
    <Svg>
      <path d="M2.75 3.25h10.5v7H8.5L5.25 13v-2.75H2.75z" />
    </Svg>
  );
}

/** Sort: lines that grow, with an arrow pointing down. */
export function SortIcon() {
  return (
    <Svg>
      <path d="M2.5 4h4M2.5 8h6M2.5 12h8" />
      <path d="M13 3v9.5M10.75 10.25 13 12.5l2.25-2.25" />
    </Svg>
  );
}

/** View settings: two sliders. */
export function SlidersIcon() {
  return (
    <Svg>
      <path d="M2.5 5h5.25M12.25 5h1.25M2.5 11h1.25M8.25 11h5.25" />
      <circle cx="10" cy="5" r="2.25" />
      <circle cx="6" cy="11" r="2.25" />
    </Svg>
  );
}
