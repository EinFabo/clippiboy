/** Text fields where a menu of our own with paste makes sense. */
const TEXT_TYPES = ["text", "search", "url", "email", "tel", "password", "number"];

export type TextField = HTMLInputElement | HTMLTextAreaElement;

/**
 * Is the target a field you can type into?
 *
 * Two places ask: the global text field menu, and the clip tile, which holds
 * back its own menu while the cursor is over its name field.
 */
export function isTextField(target: EventTarget | null): target is TextField {
  if (target instanceof HTMLTextAreaElement) return true;
  return target instanceof HTMLInputElement && TEXT_TYPES.includes(target.type);
}
