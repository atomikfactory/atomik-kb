// The small icons of the template UI (ADR-0056). Like the others they follow the text colour and are decorative: the control
// that holds one carries the accessible name.

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

/** A page with a dashed outline: the Templates entry, the template mark. */
export function TemplateIcon() {
  return (
    <Svg>
      <path d="M4 1.75h5.25L12.5 5v7.25a1 1 0 0 1-1 1h-7.5a1 1 0 0 1-1-1v-9.5a1 1 0 0 1 1-1Z" strokeDasharray="2 1.6" />
      <path d="M9 1.75V5h3.5" />
      <path d="M5.5 8.5h5M5.5 10.75h3" />
    </Svg>
  );
}

/** A page with a plus: "New page from template…". */
export function PageFromTemplateIcon() {
  return (
    <Svg>
      <path d="M8.5 13.75h-4a1 1 0 0 1-1-1v-9.5a1 1 0 0 1 1-1h3.75L11.5 5.5v2" />
      <path d="M8.25 2.75V5.5H11.5" />
      <path d="M11.5 10.25v4M9.5 12.25h4" />
    </Svg>
  );
}

/** A sparkle: the mark of a value that is filled in when a page is made from the template. */
export function FilledInIcon() {
  return (
    <Svg>
      <path d="M8 2.25 9.35 6.65 13.75 8 9.35 9.35 8 13.75 6.65 9.35 2.25 8 6.65 6.65Z" />
    </Svg>
  );
}
