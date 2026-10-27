/** Hosts that appear in the bundle as text and are never requested. */
export const INERT_HOSTS: ReadonlySet<string>;
/** The problems of a build output directory against the viewer's policy; none means it fits. */
export function checkDist(dir: string): string[];
