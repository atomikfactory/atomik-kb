// The worker code of pdf.js has no types of its own; the engine's tests run it in their own thread
// (pdf-engine.test.ts), and the app loads it as a URL (`?url`, css-modules.d.ts).
declare module "pdfjs-dist/legacy/build/pdf.worker.mjs";
