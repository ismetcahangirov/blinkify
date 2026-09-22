/**
 * Join class names, dropping anything falsy.
 *
 * Six lines rather than `clsx`. `CLAUDE.md` section 10 rule 3: a dependency for
 * something a competent developer writes in forty lines is not worth the supply
 * chain risk, and this is not forty.
 */
export function classNames(
  ...parts: readonly (string | false | null | undefined)[]
): string {
  return parts.filter((part): part is string => Boolean(part)).join(" ");
}
