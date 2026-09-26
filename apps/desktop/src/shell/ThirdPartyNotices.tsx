import { Button, Dialog, DialogClose, ScrollArea } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

/**
 * Help ▸ Third-party notices (#73): the attribution document the installer
 * put beside the application, shown as it is.
 *
 * The text is read from the installed file by the shell rather than bundled
 * into the renderer, so what the user reads is the file that shipped — the
 * one `pnpm attribution:check` keeps current — and it needs no network. The
 * renderer only lays it out.
 */
export function ThirdPartyNotices({
  open,
  onOpenChange,
}: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) {
  const [text, setText] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    if (!open || text !== null) return;
    let cancelled = false;
    invoke<string>("third_party_notices").then(
      (notices) => {
        if (!cancelled) setText(notices);
      },
      (error: unknown) => {
        if (!cancelled) setProblem(String(error));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [open, text]);

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Third-party notices"
      description="The copyright notices and licences of the software Blinkify is built from and ships with."
      className="third-party-notices"
      actions={
        <DialogClose>
          <Button variant="primary">Close</Button>
        </DialogClose>
      }
    >
      {problem !== null ? (
        <p className="third-party-notices__problem" role="alert">
          The notices could not be read: {problem}
        </p>
      ) : text === null ? (
        <p className="third-party-notices__loading">Reading the notices…</p>
      ) : (
        <ScrollArea className="third-party-notices__scroll">
          <pre
            className="third-party-notices__text"
            data-testid="third-party-notices"
          >
            {text}
          </pre>
        </ScrollArea>
      )}
    </Dialog>
  );
}
