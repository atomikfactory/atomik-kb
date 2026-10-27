// Pure helpers of the pairing screens (ADR-0022): reading a typed pairing code and
// showing the six digits.

/** Characters of a pairing code (Crockford base32, without separators). */
export const PAIRING_CODE_CHARS = 12;

/**
 * A typed pairing code as the core takes it: separators (spaces, dashes, dots, underscores)
 * dropped and letters upper-cased, so that "abcd-efgh jkmn" works. The core checks the
 * alphabet and the checksum (and reads O as 0, I and L as 1).
 */
export function normalizePairingCode(typed: string): string {
  return typed.replace(/[\s\-._]/gu, "").toUpperCase();
}

/** The code in three groups of four, as the joiner shows it ("ABCD EFGH JKMN"). */
export function pairingCodeGroups(code: string): string[] {
  const plain = normalizePairingCode(code);
  const groups: string[] = [];
  for (let at = 0; at < plain.length; at += 4) {
    groups.push(plain.slice(at, at + 4));
  }
  return groups;
}

/** The six digits in two groups of three ("123 456"), easier to compare aloud. */
export function digitGroups(sas: string): string[] {
  return sas.length === 6 ? [sas.slice(0, 3), sas.slice(3)] : [sas];
}

