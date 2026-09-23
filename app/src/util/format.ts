export const short = (oid: string | null | undefined) => (oid ? oid.slice(0, 7) : "");

/** "just now", "5m ago", "3h ago", "2d ago", or a date. */
export function ago(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "never";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const s = Math.round((now - t) / 1000);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  if (s < 86400 * 14) return `${Math.round(s / 86400)}d ago`;
  return new Date(t).toLocaleDateString();
}

export function plural(n: number, word: string, many = `${word}s`) {
  return `${n} ${n === 1 ? word : many}`;
}

const imageTypes: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  bmp: "image/bmp",
  ico: "image/x-icon",
};

export function imageType(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return imageTypes[ext] ?? null;
}

/** "now", "in 30s", "in 4m" for a future time. */
export function until(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.round((t - now) / 1000);
  if (s <= 1) return "now";
  if (s < 90) return `in ${s}s`;
  if (s < 5400) return `in ${Math.round(s / 60)}m`;
  return `in ${Math.round(s / 3600)}h`;
}
