/** Treat sqlite/json/switch values as a real on/off flag. */
export function flagOn(v, fallback = false) {
  if (v === undefined || v === null || v === '') return fallback;
  return v !== false && v !== 0 && v !== 'false' && v !== '0';
}

export function flagClass(on) {
  return (
    'inline-flex cursor-pointer select-none items-center gap-2 rounded-lg border px-3 py-2 text-sm shadow-sm transition-colors ' +
    (on ? 'border-primary bg-primary/10' : 'border-foreground/25 bg-background hover:bg-muted')
  );
}
