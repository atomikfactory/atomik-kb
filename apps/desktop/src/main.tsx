import "@atomik-kb/ui/styles.css";

import { TauriTransport } from "@atomik-kb/core-client/tauri";
import { App } from "@atomik-kb/ui";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

const container = document.getElementById("root");
if (container === null) {
  throw new Error("index.html is missing the #root element");
}

// The shell owns the core; the UI only ever sees the transport.
const transport = new TauriTransport();

createRoot(container).render(
  <StrictMode>
    <App transport={transport} />
  </StrictMode>,
);
