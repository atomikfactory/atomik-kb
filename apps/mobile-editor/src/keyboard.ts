// Tracks the soft keyboard through `visualViewport` and publishes how much of the layout viewport it covers as
// `--kb-inset` on the root element, so the toolbar can sit right above it whether the WebView resizes its layout (inset 0,
// the toolbar is at the bottom of a smaller window) or only its visual viewport.

import { useEffect, useState } from "react";

/** The dev harness fakes a keyboard with `?keyboard=<px>`; a real device never has the parameter. */
function fakeInset(): number | null {
  if (!import.meta.env.DEV) {
    return null;
  }
  const value = Number(new URLSearchParams(location.search).get("keyboard"));
  return Number.isFinite(value) && value > 0 ? value : null;
}

export function useKeyboardInset(onChange?: () => void): number {
  const [inset, setInset] = useState(0);
  useEffect(() => {
    const viewport = window.visualViewport;
    const root = document.documentElement;
    const update = () => {
      const faked = fakeInset();
      const measured =
        viewport === null ? 0 : Math.max(0, Math.round(window.innerHeight - viewport.height - viewport.offsetTop));
      const next = faked ?? measured;
      root.style.setProperty("--kb-inset", `${next}px`);
      setInset(next);
      onChange?.();
    };
    update();
    viewport?.addEventListener("resize", update);
    viewport?.addEventListener("scroll", update);
    window.addEventListener("resize", update);
    return () => {
      viewport?.removeEventListener("resize", update);
      viewport?.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, [onChange]);
  return inset;
}
