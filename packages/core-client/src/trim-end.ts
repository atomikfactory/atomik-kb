/** `text` without the run of characters of `chars` that ends it, in one pass (a regex `[...]+$` is quadratic on a long run that does not end the text). */
export function trimEndChars(text: string, chars: string): string {
  let end = text.length;
  while (end > 0 && chars.includes(text.charAt(end - 1))) {
    end -= 1;
  }
  return text.slice(0, end);
}
