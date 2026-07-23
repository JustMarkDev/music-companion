type KeyboardShortcut = Pick<KeyboardEvent, "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "code">;

export function keyboardEventToAccelerator(event: KeyboardShortcut) {
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.shiftKey) parts.push("Shift");
  if (event.altKey) parts.push("Alt");
  if (event.metaKey) parts.push("Super");
  parts.push(event.code);
  return parts.join("+");
}

export function formatAccelerator(accelerator: string) {
  return accelerator
    .replace(/Key([A-Z])/g, "$1")
    .replace("ArrowRight", "Right Arrow")
    .replace("ArrowLeft", "Left Arrow")
    .replace("ArrowUp", "Up Arrow")
    .replace("ArrowDown", "Down Arrow")
    .split("+")
    .join(" + ");
}
