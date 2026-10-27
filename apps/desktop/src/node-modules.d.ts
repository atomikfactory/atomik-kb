// The Node.js functions vite.config.ts uses (the `excalidrawAssets` plugin). The desktop package has
// no Node.js types (the app runs in a webview); the build config runs in Node.js under Vite.
declare module "node:fs" {
  export function createReadStream(path: string): { pipe: (destination: unknown) => unknown };
  export function existsSync(path: string): boolean;
  export function readdirSync(path: string): string[];
  export function readFileSync(path: string, encoding: "utf8"): string;
  export function readFileSync(path: string): Uint8Array;
  export function statSync(path: string): { isDirectory: () => boolean; isFile: () => boolean };
}
declare module "node:module" {
  export function createRequire(path: string): { resolve: (request: string) => string };
}
declare module "node:path" {
  export function dirname(path: string): string;
  export function join(...parts: string[]): string;
  export function relative(from: string, to: string): string;
  export function resolve(...parts: string[]): string;
  export const sep: string;
}
declare module "node:url" {
  export function fileURLToPath(url: string | URL): string;
}
