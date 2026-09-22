import type { ComponentPropsWithRef } from "react";
import { classNames } from "./classNames.js";

/**
 * The Blinkify button.
 *
 * ── On the primary variant, and the requirement it does not meet ────────────
 *
 * #17 asks for `primary` to carry the brand gradient. It does not, and the
 * reason is measured rather than aesthetic.
 *
 * No label colour clears WCAG AA across the gradient's sweep. White measures
 * 2.72:1 against the azure stop — it fails even the 3:1 that large text owes —
 * and near-black measures 3.67:1 against the indigo stop. There is no third
 * option: a label is one colour and the surface under it is three.
 *
 * So `primary` uses the flat `--accent`, whose three interaction states were
 * measured for exactly this job in #15: 5.33:1 resting, 4.71:1 hovered, 6.46:1
 * pressed. The gradient keeps the surfaces where it carries no text — the logo,
 * the playhead handle, selection accents.
 *
 * `components.test.tsx` asserts the measurement rather than this comment, so if
 * the brand ever moves far enough for a label to be legible on the gradient,
 * the decision is reopened by a failing test rather than by somebody
 * remembering it was made.
 */

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ControlSize = "sm" | "md" | "lg";

export interface ButtonProps extends ComponentPropsWithRef<"button"> {
  /**
   * `primary` for the one action a view is about, `secondary` for the rest,
   * `ghost` inside a toolbar where a border per control would be a grid, and
   * `danger` for a destructive action.
   *
   * The default is `secondary` rather than `primary`, because most buttons in
   * an editor are not the primary action and a default that has to be argued
   * out of is a default that produces a screen full of primaries.
   */
  readonly variant?: ButtonVariant;
  readonly size?: ControlSize;
  /**
   * Work is in progress. The button stops responding and keeps its width — the
   * label goes invisible rather than being removed, because a button that
   * narrows when clicked moves every control beside it, and in a toolbar that
   * means the next thing the user was about to click has moved.
   */
  readonly loading?: boolean;
}

export function Button({
  variant = "secondary",
  size = "md",
  loading = false,
  className,
  children,
  disabled,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      // `button`, not the HTML default of `submit`. A primitive that submits a
      // form nobody knew it was in is a bug that only appears once there is a
      // form, which is long after this component was reviewed.
      type={type}
      className={classNames(
        "bk-button",
        `bk-button--${variant}`,
        size !== "md" && `bk-button--${size}`,
        className,
      )}
      disabled={disabled === true || loading}
      data-loading={loading ? "true" : undefined}
      // `aria-busy` rather than only `disabled`: a screen reader user needs to
      // know the control is working, not merely that it stopped answering.
      aria-busy={loading || undefined}
      {...rest}
    >
      <span className="bk-button__label">{children}</span>
      {loading ? (
        // Essential motion. A frozen spinner says the application hung, so this
        // is one of the few things that keeps moving under
        // `prefers-reduced-motion` — see the foot of tokens/scales.css.
        <span
          className="bk-button__spinner"
          data-motion="essential"
          aria-hidden="true"
        />
      ) : null}
    </button>
  );
}
