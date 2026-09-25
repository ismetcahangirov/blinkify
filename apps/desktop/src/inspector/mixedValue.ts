/**
 * The inspector's one rule for a selection of several clips (#56, and #49
 * on the audio side): a property the clips agree on shows its value; one
 * they disagree on shows as mixed, and is never silently replaced by the
 * first clip's value. Setting a mixed property is an explicit act that
 * applies the new value to every selected clip.
 */
export type Shared<T> =
  | { readonly kind: "none" }
  | { readonly kind: "same"; readonly value: T }
  | { readonly kind: "mixed" };

export function shared<T>(
  values: readonly T[],
  equal: (a: T, b: T) => boolean = Object.is,
): Shared<T> {
  const [first, ...rest] = values;
  if (first === undefined) return { kind: "none" };
  return rest.every((value) => equal(first, value))
    ? { kind: "same", value: first }
    : { kind: "mixed" };
}
