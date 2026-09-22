import * as RadixDialog from "@radix-ui/react-dialog";
import type { ReactNode } from "react";
import { classNames } from "./classNames.js";

/**
 * A modal dialog.
 *
 * ── Why `title` is a required prop ──────────────────────────────────────────
 *
 * A dialog with no accessible name is announced as "dialog", and the user who
 * cannot see it has no idea what has just taken over the window. Radix warns
 * about a missing `Dialog.Title` in development and does nothing about it in
 * production, which is a warning nobody reads. Making it a prop means it cannot
 * be forgotten and cannot be styled away.
 *
 * ── Why the description is separate from the body ──────────────────────────
 *
 * `Dialog.Description` is wired to `aria-describedby`, so it is read out
 * immediately after the title. The body is not. "This will re-encode three
 * segments" belongs in the description; a table of those segments belongs in
 * the body.
 *
 * ── Use it sparingly ───────────────────────────────────────────────────────
 *
 * A modal stops the user doing the thing they opened the application to do.
 * Blinkify has two that earn it — the export dialog and the confirmation before
 * overwriting an export target (`CLAUDE.md` section 19). The update offer
 * deliberately is not one: it is information, not an interruption, so it is a
 * banner.
 */

export interface DialogProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  /** Required. Becomes the accessible name. */
  readonly title: string;
  /** One sentence, read out after the title. */
  readonly description?: string;
  readonly children?: ReactNode;
  /** The buttons, right-aligned. Primary action last, as Windows orders them. */
  readonly actions?: ReactNode;
  readonly className?: string;
}

export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  children,
  actions,
  className,
}: DialogProps) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="bk-dialog__overlay" />
        <RadixDialog.Content
          className={classNames("bk-surface", "bk-dialog__content", className)}
        >
          <RadixDialog.Title className="bk-dialog__title">
            {title}
          </RadixDialog.Title>
          {description === undefined ? null : (
            <RadixDialog.Description className="bk-dialog__description">
              {description}
            </RadixDialog.Description>
          )}
          {children}
          {actions === undefined ? null : (
            <div className="bk-dialog__actions">{actions}</div>
          )}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}

/**
 * Closes the dialog it is inside, whatever the button is.
 *
 * Wrap a `Button` in it rather than calling `onOpenChange(false)` by hand: the
 * Radix close is what returns focus to whatever opened the dialog, and a
 * hand-rolled close drops the keyboard user back at the top of the document.
 */
export function DialogClose({ children }: { readonly children: ReactNode }) {
  return <RadixDialog.Close asChild>{children}</RadixDialog.Close>;
}
