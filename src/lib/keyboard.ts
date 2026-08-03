/// Whether a keystroke is destined for a real text input, so bare shortcuts
/// (`i`, Escape) must not hijack it. Read-only editor bodies are not
/// contenteditable, so they never match.
export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable;
}
