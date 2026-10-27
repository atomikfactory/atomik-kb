// The Node.js file functions the fixture generator, the fixture tests and the language file tests use. The UI
// package has no Node.js types (it runs in browsers); tests run in Node.js under Vitest.
declare module "node:fs" {
  export function readFileSync(path: URL | string, encoding: "utf8"): string;
  export function readFileSync(path: URL | string): Uint8Array;
  export function writeFileSync(path: URL | string, data: string | Uint8Array): void;
  export function readdirSync(path: URL | string): string[];
}
