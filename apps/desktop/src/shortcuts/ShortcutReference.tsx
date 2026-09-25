import { Button, Dialog, DialogClose } from "@blinkify/ui";
import { reference } from "./shortcuts.js";
import { useShortcutsUi } from "./shortcuts.store.js";

/**
 * The keyboard shortcut reference (#38): Ctrl+/ or Help ▸ Keyboard
 * shortcuts. Generated from the registry, so it cannot list a key that does
 * something else, or miss one.
 */
export function ShortcutReference() {
  const open = useShortcutsUi((state) => state.referenceOpen);
  const setOpen = useShortcutsUi((state) => state.setReferenceOpen);
  return (
    <Dialog
      open={open}
      onOpenChange={setOpen}
      title="Keyboard shortcuts"
      description="CapCut's keys wherever CapCut has one. Keys do nothing while you type in a field."
      className="shortcut-reference"
      actions={
        <DialogClose>
          <Button variant="primary">Close</Button>
        </DialogClose>
      }
    >
      {reference().map(({ group, rows }) => (
        <section key={group} aria-labelledby={`shortcuts-${group}`}>
          <h3 id={`shortcuts-${group}`} className="shortcut-reference__group">
            {group}
          </h3>
          <table className="shortcut-reference__table">
            <tbody>
              {rows.map((row) => (
                <tr key={row.action} data-action={row.action}>
                  <th scope="row">{row.label}</th>
                  <td>
                    {row.keys.map((key) => (
                      <kbd key={key}>{key}</kbd>
                    ))}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ))}
    </Dialog>
  );
}
