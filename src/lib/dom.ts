/** Textfelder, in denen ein eigenes Menü mit Einfügen sinnvoll ist. */
const TEXT_TYPES = ["text", "search", "url", "email", "tel", "password", "number"];

export type TextField = HTMLInputElement | HTMLTextAreaElement;

/**
 * Ist das Ziel ein Feld, in das man schreiben kann?
 *
 * Zwei Stellen fragen danach: das globale Textfeld-Menü und die Clip-Kachel,
 * die ihr eigenes Menü zurückhält, solange der Zeiger über ihrem Namensfeld
 * steht.
 */
export function isTextField(target: EventTarget | null): target is TextField {
  if (target instanceof HTMLTextAreaElement) return true;
  return target instanceof HTMLInputElement && TEXT_TYPES.includes(target.type);
}
