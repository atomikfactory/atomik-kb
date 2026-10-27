import { installEditorStyles } from "@atomik-kb/ui";
import "@atomik-kb/ui/styles.css";
import "./viewer.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { browserLanguages, pickLanguage } from "./messages";
import { StatusScreen, Viewer } from "./Viewer";
import { ViewerTransport } from "./ViewerTransport";
import { loadKbWasm } from "./wasm";

// The viewer of a view link (M13 slice 4, ADR-0051). The address is `<server>/view#<key>`: the key is what follows the
// `#`, the browser never sends it to the server, and this file takes it out of the address bar (`history.replaceState`,
// so neither the session's history nor a later copy of the address holds it) before anything else runs. It lives in this
// script's memory from then on; a reload therefore needs the full link again.
const fragment = location.hash;
history.replaceState(null, "", `${location.pathname}${location.search}`);

const container = document.getElementById("root");
if (container === null) {
  throw new Error("index.html is missing the #root element");
}
// The editor writes no inline <style> (the policy refuses one); its rules come from a constructable stylesheet.
installEditorStyles();
const language = pickLanguage(browserLanguages());
document.documentElement.lang = language;
const root = createRoot(container);

if (fragment === "" || fragment === "#") {
  root.render(
    <StrictMode>
      <StatusScreen kind="nokey" language={language} />
    </StrictMode>,
  );
} else {
  loadKbWasm().then(
    (wasm) => {
      const transport = new ViewerTransport({ fragment, wasm });
      root.render(
        <StrictMode>
          <Viewer transport={transport} language={language} />
        </StrictMode>,
      );
    },
    () => {
      // The module did not load (a server without the file, a browser without WebAssembly): the page cannot be opened.
      root.render(
        <StrictMode>
          <StatusScreen kind="failed" language={language} />
        </StrictMode>,
      );
    },
  );
}
