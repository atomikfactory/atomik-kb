// The Node.js functions the viewer's tests use. The viewer package has no Node.js types (the app runs in a browser);
// the tests run in Node.js under Vitest.
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
  export function readFileSync(path: string): Uint8Array;
  export function existsSync(path: string): boolean;
}
declare module "node:path" {
  export function join(...parts: string[]): string;
}
declare module "node:url" {
  export function fileURLToPath(url: string | URL): string;
}
