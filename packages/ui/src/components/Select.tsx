import * as RadixSelect from "@radix-ui/react-select";
import { classNames } from "./classNames.js";
import { FLOATING_OFFSET_PX } from "./Tooltip.js";

/**
 * A select, taking its options as data rather than as children.
 *
 * ── Why an items array and not compound components ─────────────────────────
 *
 * Every select in Blinkify has a closed, short, known list: preview quality is
 * full, half and quarter; aspect ratio is a handful of sequence presets; an
 * export preset is whatever the planner offers. None of them wants arbitrary
 * children, and a compound API would spend twenty lines of markup at every call
 * site to express a list of three strings.
 *
 * If a case ever arrives that genuinely needs custom option content, that is
 * the moment to add the compound form — not before, and not on the chance.
 *
 * Radix supplies typeahead, roving focus, escape handling, portal behaviour and
 * the ARIA wiring, all of which are where a hand-rolled dropdown goes wrong.
 */

export interface SelectOption {
  readonly value: string;
  readonly label: string;
  readonly disabled?: boolean;
}

export interface SelectProps {
  /** The accessible name. Required: "combo box" is not a name. */
  readonly label: string;
  readonly value: string;
  readonly onValueChange: (value: string) => void;
  readonly options: readonly SelectOption[];
  readonly disabled?: boolean;
  readonly className?: string;
}

/**
 * The tick beside the chosen option, and the caret on the trigger.
 *
 * Inline rather than from an icon set, because two paths are not an icon set
 * and #17 ships no icon dependency. `currentColor` so both follow the text they
 * sit beside, including into the disabled and highlighted states.
 */
function CheckGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M3.5 8.5l3 3 6-7"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function CaretGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M4 6.5l4 4 4-4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function Select({
  label,
  value,
  onValueChange,
  options,
  disabled = false,
  className,
}: SelectProps) {
  return (
    <RadixSelect.Root
      value={value}
      onValueChange={onValueChange}
      disabled={disabled}
    >
      <RadixSelect.Trigger
        className={classNames("bk-select__trigger", className)}
        aria-label={label}
      >
        <RadixSelect.Value />
        <RadixSelect.Icon className="bk-select__icon">
          <CaretGlyph />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>
      <RadixSelect.Portal>
        <RadixSelect.Content
          className="bk-surface bk-menu"
          /* `popper`, not Radix's default `item-aligned`. Item-aligned places
             the list over the trigger so the current value stays under the
             pointer, which is native macOS behaviour and looks like a rendering
             fault on Windows, where every list opens below its control. */
          position="popper"
          sideOffset={FLOATING_OFFSET_PX}
        >
          <RadixSelect.Viewport className="bk-select__viewport">
            {options.map((option) => (
              <RadixSelect.Item
                key={option.value}
                className="bk-menu__item"
                value={option.value}
                disabled={option.disabled ?? false}
              >
                <span className="bk-menu__indicator">
                  <RadixSelect.ItemIndicator>
                    <CheckGlyph />
                  </RadixSelect.ItemIndicator>
                </span>
                <RadixSelect.ItemText>{option.label}</RadixSelect.ItemText>
              </RadixSelect.Item>
            ))}
          </RadixSelect.Viewport>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  );
}
