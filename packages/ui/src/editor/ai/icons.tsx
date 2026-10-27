// The two glyphs of the editor's AI entries in the `/` menu, in the style of components/icons.tsx
// (16 px, the text colour, decorative).

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

/** A spark: write or edit with AI. */
export function SparkleIcon() {
  return (
    <Svg>
      <path d="M7 2.5 8.4 6 12 7.5 8.4 9 7 12.5 5.6 9 2 7.5 5.6 6Z" />
      <path d="M12.5 1.5v3M11 3h3" />
    </Svg>
  );
}

/** A microphone: record audio. */
export function RecordIcon() {
  return (
    <Svg>
      <rect x="6" y="2" width="4" height="7" rx="2" />
      <path d="M3.75 7.5a4.25 4.25 0 0 0 8.5 0M8 11.75V14" />
    </Svg>
  );
}
