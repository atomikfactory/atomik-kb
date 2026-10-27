/**
 * Writes `text` to the clipboard; rejects when the clipboard is missing or refuses. Callers
 * fall back to showing the text in a `TextDialog` to copy by hand.
 */
export async function writeClipboard(text: string): Promise<void> {
  const clipboard = (navigator as { clipboard?: Clipboard }).clipboard;
  if (clipboard === undefined) {
    throw new Error("no clipboard");
  }
  await clipboard.writeText(text);
}
