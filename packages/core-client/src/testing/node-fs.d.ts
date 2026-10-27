// Typings for the few Node built-ins the fixture tests use. This package does not depend on
// @types/node on purpose (its library code runs in webviews and workers, never in Node);
// `tsconfig.build.json` excludes `src/testing`, so only tests can import these modules.

declare module "node:fs" {
  export function existsSync(path: URL): boolean;
  export function readFileSync(path: URL, encoding: "utf8"): string;
  export function readFileSync(path: URL): Uint8Array;
  export function writeFileSync(path: URL, data: Uint8Array): void;
}
