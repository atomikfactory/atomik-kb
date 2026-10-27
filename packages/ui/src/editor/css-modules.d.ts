// The editor loads KaTeX's stylesheet lazily, with `import("katex/dist/katex.min.css")` (math.ts),
// and the PDF viewer's worker as a URL, with `import("pdfjs-dist/.../pdf.worker.min.mjs?url")`
// (pdf-engine.ts). The bundler handles the files; this only tells the type checker that a `.css`
// specifier and a `?url` specifier are modules (apps/desktop gets the same from `vite/client`).
declare module "*.css";
declare module "*?url" {
  const url: string;
  export default url;
}
