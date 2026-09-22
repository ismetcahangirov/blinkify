import * as RadixTabs from "@radix-ui/react-tabs";
import type { ReactNode } from "react";
import { classNames } from "./classNames.js";

/**
 * Tabs.
 *
 * The library panel's tab row is the reason this exists — Media, Audio, and
 * whatever the shell grows later (see
 * `docs/design/capcut-layout-reference.md`). It is also what the inspector
 * would use if it ever needed one, which it must not: the inspector's content
 * is driven entirely by the timeline selection, and a tab there would give the
 * user two places to look for one answer.
 *
 * Radix supplies roving focus: one Tab stop for the whole row, arrow keys to
 * move between tabs. That is the correct behaviour and it is not what a row of
 * buttons does — a row of buttons puts eight stops between the user and the
 * panel below.
 */

export interface TabDefinition {
  readonly value: string;
  readonly label: ReactNode;
  readonly content: ReactNode;
  readonly disabled?: boolean;
}

export interface TabsProps {
  /** Names the row for a screen reader: "Library", not "Tabs". */
  readonly label: string;
  readonly tabs: readonly TabDefinition[];
  readonly value: string;
  readonly onValueChange: (value: string) => void;
  readonly className?: string;
}

export function Tabs({
  label,
  tabs,
  value,
  onValueChange,
  className,
}: TabsProps) {
  return (
    <RadixTabs.Root
      className={classNames("bk-tabs", className)}
      value={value}
      onValueChange={onValueChange}
    >
      <RadixTabs.List className="bk-tabs__list" aria-label={label}>
        {tabs.map((tab) => (
          <RadixTabs.Trigger
            key={tab.value}
            className="bk-tabs__trigger"
            value={tab.value}
            disabled={tab.disabled ?? false}
          >
            {tab.label}
          </RadixTabs.Trigger>
        ))}
      </RadixTabs.List>
      {tabs.map((tab) => (
        <RadixTabs.Content
          key={tab.value}
          className="bk-tabs__content"
          value={tab.value}
        >
          {tab.content}
        </RadixTabs.Content>
      ))}
    </RadixTabs.Root>
  );
}
