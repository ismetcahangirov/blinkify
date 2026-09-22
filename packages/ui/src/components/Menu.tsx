import * as RadixContextMenu from "@radix-ui/react-context-menu";
import * as RadixDropdownMenu from "@radix-ui/react-dropdown-menu";
import type { ReactNode } from "react";
import { classNames } from "./classNames.js";
import { FLOATING_OFFSET_PX } from "./Tooltip.js";

/**
 * Menus: the one that opens from a button, and the one that opens on right
 * click.
 *
 * ── Why they are one file ───────────────────────────────────────────────────
 *
 * They are two Radix packages and one design. A dropdown menu and a context
 * menu differ in how they are summoned and in nothing the user can see
 * afterwards — same items, same highlight, same shortcuts, same separators.
 * Built in separate files they drift, because a change made to one is a change
 * somebody forgot to make to the other, and the day they differ is the day the
 * interface starts feeling assembled rather than designed.
 *
 * So the item shape is declared once, as data, and each menu renders it with
 * its own package's components. That is also why the item type is exported:
 * the timeline's context menu and the Edit menu offer overlapping commands, and
 * they should be able to share the list.
 *
 * ── Why the highlight is `data-highlighted` and not `:hover` ───────────────
 *
 * Radix sets `data-highlighted` for both the pointer and the keyboard. Styling
 * `:hover` means arrowing through a menu highlights nothing, which is what
 * people mean when they say a menu "is not keyboard accessible" while every key
 * in it works perfectly.
 */

export interface MenuItemDefinition {
  readonly id: string;
  readonly label: ReactNode;
  readonly onSelect: () => void;
  /** Shown right-aligned and dim. Display only — the binding lives in #38. */
  readonly shortcut?: string;
  readonly disabled?: boolean;
  /** Destructive. Styled apart so a delete is not one row from a rename. */
  readonly danger?: boolean;
}

export interface MenuGroupDefinition {
  /** Optional heading. Omit for an unlabelled group between separators. */
  readonly label?: string;
  readonly items: readonly MenuItemDefinition[];
}

export interface MenuProps {
  readonly groups: readonly MenuGroupDefinition[];
  readonly className?: string;
}

interface MenuPrimitives {
  readonly Item: typeof RadixDropdownMenu.Item | typeof RadixContextMenu.Item;
  readonly Label:
    typeof RadixDropdownMenu.Label | typeof RadixContextMenu.Label;
  readonly Separator:
    typeof RadixDropdownMenu.Separator | typeof RadixContextMenu.Separator;
}

/**
 * Render the groups with whichever package's primitives were handed in.
 *
 * The two packages expose the same component shapes, which is what makes one
 * renderer possible — and it is checked by the types rather than assumed.
 */
function renderGroups(
  groups: readonly MenuGroupDefinition[],
  { Item, Label, Separator }: MenuPrimitives,
): ReactNode {
  return groups.map((group, index) => (
    <div key={group.label ?? `group-${String(index)}`} role="none">
      {index > 0 ? <Separator className="bk-menu__separator" /> : null}
      {group.label === undefined ? null : (
        <Label className="bk-menu__label">{group.label}</Label>
      )}
      {group.items.map((item) => (
        <Item
          key={item.id}
          className={classNames(
            "bk-menu__item",
            item.danger === true && "bk-menu__item--danger",
          )}
          disabled={item.disabled ?? false}
          onSelect={item.onSelect}
        >
          {item.label}
          {item.shortcut === undefined ? null : (
            <span className="bk-menu__shortcut">{item.shortcut}</span>
          )}
        </Item>
      ))}
    </div>
  ));
}

export interface DropdownMenuProps extends MenuProps {
  /** The control that opens it. */
  readonly trigger: ReactNode;
  readonly align?: RadixDropdownMenu.DropdownMenuContentProps["align"];
}

export function DropdownMenu({
  trigger,
  groups,
  align = "start",
  className,
}: DropdownMenuProps) {
  return (
    <RadixDropdownMenu.Root>
      <RadixDropdownMenu.Trigger asChild>{trigger}</RadixDropdownMenu.Trigger>
      <RadixDropdownMenu.Portal>
        <RadixDropdownMenu.Content
          className={classNames("bk-surface", "bk-menu", className)}
          align={align}
          sideOffset={FLOATING_OFFSET_PX}
        >
          {renderGroups(groups, {
            Item: RadixDropdownMenu.Item,
            Label: RadixDropdownMenu.Label,
            Separator: RadixDropdownMenu.Separator,
          })}
        </RadixDropdownMenu.Content>
      </RadixDropdownMenu.Portal>
    </RadixDropdownMenu.Root>
  );
}

export interface ContextMenuProps extends MenuProps {
  /** The region that answers a right click — a clip, a track, an asset. */
  readonly children: ReactNode;
}

export function ContextMenu({ children, groups, className }: ContextMenuProps) {
  return (
    <RadixContextMenu.Root>
      <RadixContextMenu.Trigger asChild>{children}</RadixContextMenu.Trigger>
      <RadixContextMenu.Portal>
        <RadixContextMenu.Content
          className={classNames("bk-surface", "bk-menu", className)}
        >
          {renderGroups(groups, {
            Item: RadixContextMenu.Item,
            Label: RadixContextMenu.Label,
            Separator: RadixContextMenu.Separator,
          })}
        </RadixContextMenu.Content>
      </RadixContextMenu.Portal>
    </RadixContextMenu.Root>
  );
}
