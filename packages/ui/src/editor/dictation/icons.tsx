// The glyph of `/microphone` in the `/` menu, in the style of components/icons.tsx (16 px, the text colour, decorative).

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

/** A microphone with lines of text coming out of it: speech that becomes text at the cursor. */
export function DictateIcon() {
  return (
    <Svg>
      <rect x="4.25" y="1.75" width="3.5" height="6.5" rx="1.75" />
      <path d="M2.25 7a3.75 3.75 0 0 0 7.5 0M6 10.75V14" />
      <path d="M12 4h2.5M12 7h2.5M12 10h2.5" />
    </Svg>
  );
}
