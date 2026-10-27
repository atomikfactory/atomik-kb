// Small inline icons of the properties UI: one glyph per kind of property and the few controls of
// the panel and the dialog. Like `icons.tsx`, all are decorative: the control that holds one carries
// the accessible name.

import type { PropertyKind } from "@atomik-kb/core-client";
import type { SVGProps } from "react";

import { AtIcon, CalendarIcon, CheckIcon, FileIcon, LinkIcon, TextIcon } from "./icons";

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

/** Number: a hash sign. */
function NumberGlyph() {
  return (
    <Svg>
      <path d="M6 2.5 5 13.5M11 2.5l-1 11M2.75 6h10.5M2.25 10h10.5" />
    </Svg>
  );
}

/** Select: a circle with a chevron. */
function SelectGlyph() {
  return (
    <Svg>
      <circle cx="8" cy="8" r="6.25" />
      <path d="m5.5 7 2.5 2.5L10.5 7" />
    </Svg>
  );
}

/** Multi-select: a list of three bullets. */
function MultiSelectGlyph() {
  return (
    <Svg>
      <path d="M6.5 4h7M6.5 8h7M6.5 12h7" />
      <circle cx="3" cy="4" r="0.9" fill="currentColor" stroke="none" />
      <circle cx="3" cy="8" r="0.9" fill="currentColor" stroke="none" />
      <circle cx="3" cy="12" r="0.9" fill="currentColor" stroke="none" />
    </Svg>
  );
}

/** Status: a circle that is half full. */
function StatusGlyph() {
  return (
    <Svg>
      <circle cx="8" cy="8" r="6.25" />
      <path d="M8 4.25a3.75 3.75 0 0 1 0 7.5z" fill="currentColor" stroke="none" />
    </Svg>
  );
}

/** Phone: a handset. */
function PhoneGlyph() {
  return (
    <Svg>
      <path d="M4.25 2.5h2l1 2.75-1.5 1a7.5 7.5 0 0 0 3 3l1-1.5 2.75 1v2a1.5 1.5 0 0 1-1.5 1.5A9.25 9.25 0 0 1 2.75 4a1.5 1.5 0 0 1 1.5-1.5z" />
    </Svg>
  );
}

/** Checklist: three lines, the first two ticked. */
function ChecklistGlyph() {
  return (
    <Svg>
      <path d="m2.5 4 1 1 1.75-2M2.5 8.5l1 1 1.75-2" />
      <path d="M7.5 4h6M7.5 8.5h6M7.5 13h6" />
      <circle cx="3.75" cy="13" r="1" />
    </Svg>
  );
}

/** Place: a map pin. */
function PlaceGlyph() {
  return (
    <Svg>
      <path d="M8 14.25s4.5-4.1 4.5-7.75a4.5 4.5 0 0 0-9 0C3.5 10.15 8 14.25 8 14.25z" />
      <circle cx="8" cy="6.5" r="1.6" />
    </Svg>
  );
}

/** Created or last edited time: a clock. */
function ClockGlyph() {
  return (
    <Svg>
      <circle cx="8" cy="8" r="6.25" />
      <path d="M8 4.5V8l2.25 1.5" />
    </Svg>
  );
}

/** Created by or last edited by: a person. */
function PersonGlyph() {
  return (
    <Svg>
      <circle cx="8" cy="5.5" r="2.5" />
      <path d="M3 13.5c.6-2.4 2.5-3.6 5-3.6s4.4 1.2 5 3.6" />
    </Svg>
  );
}

/** Tags: a label with a hole. */
export function TagGlyph() {
  return (
    <Svg>
      <path d="M2.5 2.5h5l6 6-5 5-6-6z" />
      <circle cx="5.5" cy="5.5" r="0.9" fill="currentColor" stroke="none" />
    </Svg>
  );
}

/** The glyph of a kind of property. */
export function KindGlyph({ kind }: { kind: PropertyKind }) {
  switch (kind) {
    case "text":
      return <TextIcon />;
    case "number":
      return <NumberGlyph />;
    case "select":
      return <SelectGlyph />;
    case "multi_select":
      return <MultiSelectGlyph />;
    case "date":
      return <CalendarIcon />;
    case "checkbox":
      return <CheckIcon />;
    case "url":
      return <LinkIcon />;
    case "status":
      return <StatusGlyph />;
    case "email":
      return <AtIcon />;
    case "phone":
      return <PhoneGlyph />;
    case "files":
      return <FileIcon />;
    case "checklist":
      return <ChecklistGlyph />;
    case "place":
      return <PlaceGlyph />;
    case "person":
      return <PersonGlyph />;
    case "relation":
      return <RelationGlyph />;
    case "rollup":
      return <RollupGlyph />;
    case "formula":
      return <FormulaGlyph />;
    case "created_time":
    case "last_edited_time":
      return <ClockGlyph />;
    case "created_by":
    case "last_edited_by":
      return <PersonGlyph />;
  }
}

/** Relation: two boxes joined by a line (pages that point at each other). */
function RelationGlyph() {
  return (
    <Svg>
      <rect x="1.75" y="2.75" width="5" height="4.5" rx="1" />
      <rect x="9.25" y="8.75" width="5" height="4.5" rx="1" />
      <path d="M6.75 5h2.5a1.5 1.5 0 0 1 1.5 1.5v2.25" />
    </Svg>
  );
}

/** Rollup: a sigma (a sum over the related pages). */
function RollupGlyph() {
  return (
    <Svg>
      <path d="M12.5 3h-9L8 8l-4.5 5h9" />
    </Svg>
  );
}

/** Formula: an italic f with a small x. */
function FormulaGlyph() {
  return (
    <Svg>
      <path d="M10 2.75c-1.75 0-2.25 1-2.5 2.5L6 12.25c-.25 1.25-.75 1.5-2 1.5M4.75 6.5h4.5M10 9l3 3.75M13 9l-3 3.75" />
    </Svg>
  );
}

/** Edit: a pencil. */
export function PencilIcon() {
  return (
    <Svg>
      <path d="m3 13 .6-2.9 6.9-6.9a1.4 1.4 0 0 1 2 0l.3.3a1.4 1.4 0 0 1 0 2L5.9 12.4z" />
      <path d="m9.5 4.3 2.2 2.2" />
    </Svg>
  );
}

/** Remove: a cross. */
export function CloseIcon() {
  return (
    <Svg>
      <path d="m4 4 8 8M12 4l-8 8" />
    </Svg>
  );
}

/** Move up: an arrow pointing up. */
export function ArrowUpIcon() {
  return (
    <Svg>
      <path d="M8 13V3M4 7l4-4 4 4" />
    </Svg>
  );
}

/** Move down: an arrow pointing down. */
export function ArrowDownIcon() {
  return (
    <Svg>
      <path d="M8 3v10M4 9l4 4 4-4" />
    </Svg>
  );
}

/** Open a link: an arrow leaving a box. */
export function OpenIcon() {
  return (
    <Svg>
      <path d="M9 2.75h4.25V7M13.25 2.75 7.5 8.5M11.5 9.5v3.25a.75.75 0 0 1-.75.75h-7a.75.75 0 0 1-.75-.75v-7a.75.75 0 0 1 .75-.75H7" />
    </Svg>
  );
}
