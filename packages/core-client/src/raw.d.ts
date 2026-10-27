// `import text from "./file?raw"` gives the file's contents as a string (Vite and Vitest feature).
declare module "*?raw" {
  const content: string;
  export default content;
}
