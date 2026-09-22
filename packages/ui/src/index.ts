/**
 * The Blinkify design system.
 *
 * Boundary rule (`CLAUDE.md` section 2, enforced by `ui-package-stays-shared`
 * in #11): this package may not import from `apps/`. A design system that
 * reaches back into an application is not a design system — it is that
 * application with extra steps.
 *
 * The tokens themselves are CSS, not TypeScript, and are consumed by importing
 * `@blinkify/ui/styles.css` — one import that brings the bundled typefaces
 * (#16), the colour tokens (#15) and the type, spacing, radius, elevation and
 * motion scales (#16), in that order. The individual stylesheets are also
 * exported, for a consumer that genuinely needs one of them alone.
 *
 * What is exported here is the machinery that proves the tokens are usable:
 * contrast measurement, and the list of pairs that have to hold.
 *
 * `fonts/fontMetrics.ts` is deliberately absent from this barrel. It reads a
 * WOFF2 binary through `node:zlib` to assert the timecode digits are all one
 * width, and re-exporting it would put a Node built-in on the path of anything
 * that imports the design system. Tests import it by its own path.
 *
 * The primitives (#17) are exported below. They are styled with plain CSS
 * classes rather than Tailwind utilities, because this package may not depend
 * on the application that configures Tailwind — and because a primitive styled
 * with the renderer's utilities would render unstyled in Storybook, which is
 * exactly where it has to be reviewable.
 */

export {
  Button,
  type ButtonProps,
  type ButtonVariant,
  type ControlSize,
} from "./components/Button.js";
export { IconButton, type IconButtonProps } from "./components/IconButton.js";
export {
  Tooltip,
  TooltipProvider,
  TOOLTIP_DELAY_MS,
  TOOLTIP_SKIP_DELAY_MS,
  FLOATING_OFFSET_PX,
  type TooltipProps,
  type TooltipProviderProps,
} from "./components/Tooltip.js";
export { Slider, type SliderProps } from "./components/Slider.js";
export {
  NumberInput,
  type NumberInputProps,
} from "./components/NumberInput.js";
export { Switch, type SwitchProps } from "./components/Switch.js";
export { Tabs, type TabsProps, type TabDefinition } from "./components/Tabs.js";
export {
  Select,
  type SelectProps,
  type SelectOption,
} from "./components/Select.js";
export { Popover, type PopoverProps } from "./components/Popover.js";
export {
  DropdownMenu,
  ContextMenu,
  type DropdownMenuProps,
  type ContextMenuProps,
  type MenuItemDefinition,
  type MenuGroupDefinition,
} from "./components/Menu.js";
export { Dialog, DialogClose, type DialogProps } from "./components/Dialog.js";
export { ScrollArea, type ScrollAreaProps } from "./components/ScrollArea.js";
export { classNames } from "./components/classNames.js";

export {
  WCAG_AA,
  contrastRatio,
  formatRatio,
  parseTokens,
  relativeLuminance,
  resolveToken,
} from "./tokens/contrast.js";

export {
  CONTRAST_PAIRS,
  DUTY_THRESHOLD,
  type ContrastDuty,
  type ContrastPair,
} from "./tokens/contrastPairs.js";

export const DESIGN_SYSTEM_VERSION = "0.1.0";
