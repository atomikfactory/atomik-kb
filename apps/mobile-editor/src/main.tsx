import "@atomik-kb/ui/styles.css";
import "./mobile.css";

import { AndroidTransport } from "@atomik-kb/core-client/android";
import { installEditorStyles } from "@atomik-kb/ui/mobile";
import { createRoot } from "react-dom/client";

import { MobileApp } from "./MobileApp";

// The editor of the Android app. Under the policy there is no inline <style>: the editor's base rules come from a
// constructable stylesheet (as in the web viewer).
installEditorStyles();

const container = document.getElementById("root");
if (container === null) {
  throw new Error("index.html is missing the #root element");
}
const root = createRoot(container);

async function start(): Promise<void> {
  // `vite dev` in a desktop browser has no app around it: a fake kbHost over an in-memory core stands in (the branch is
  // removed from the production build).
  if (import.meta.env.DEV && (window as { kbHost?: unknown }).kbHost === undefined) {
    const { installDevHost } = await import("./dev-host");
    await installDevHost();
  }
  root.render(<MobileApp transport={new AndroidTransport()} />);
}

start().catch((error: unknown) => {
  console.error("the editor could not start", error);
});
