import type { ReactNode } from "react";

/**
 * A row, for laying out several controls in one story.
 *
 * Review-only, and deliberately not exported from the package barrel: it is
 * scaffolding for Storybook, not a layout primitive. The shell's layout is #19
 * and it is a four-zone grid, not a stack of rows.
 *
 * It exists so that the story files hold no measurements. A `gap: "12px"`
 * written inline twelve times is twelve values outside the scale, in the one
 * package whose whole job is that there are no values outside the scale.
 */
export function StoryRow({
  children,
  align = "center",
  direction = "row",
}: {
  readonly children: ReactNode;
  readonly align?: "center" | "start" | "stretch";
  readonly direction?: "row" | "column";
}) {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: direction,
        alignItems: align,
        gap: "var(--space-3)",
      }}
    >
      {children}
    </div>
  );
}
