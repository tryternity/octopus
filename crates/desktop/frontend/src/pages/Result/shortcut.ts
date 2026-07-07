export function parseShortcut(s: string) {
  const parts = s.toLowerCase().split("+").map((p) => p.trim());
  const key = parts.pop();
  const cmdOrCtrl = parts.includes("cmdorctrl");
  return {
    key,
    cmdOrCtrl,
    meta: parts.includes("cmd") || parts.includes("super") || parts.includes("meta"),
    ctrl: parts.includes("control") || parts.includes("ctrl"),
    alt: parts.includes("alt") || parts.includes("option"),
    shift: parts.includes("shift"),
  };
}

export function matchShortcut(e: KeyboardEvent, sc: ReturnType<typeof parseShortcut>) {
  if (!sc || !sc.key || e.key.toLowerCase() !== sc.key) return false;
  if (sc.cmdOrCtrl) {
    if (!(e.metaKey || e.ctrlKey)) return false;
  } else {
    if (e.metaKey !== sc.meta) return false;
    if (e.ctrlKey !== sc.ctrl) return false;
  }
  return e.altKey === sc.alt && e.shiftKey === sc.shift;
}
