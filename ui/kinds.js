// What each kind of file looks like in the window, and the small formatting helpers rows and the preview share.

/** @type {Record<string, { glyph: string, label: string }>} */
export const KINDS = {
  screenshot: { glyph: "\u{f03e}", label: "Screenshot" },
  image: { glyph: "\u{f03e}", label: "Photo" },
  pdf: { glyph: "\u{f1c1}", label: "PDF" },
  doc: { glyph: "\u{f15c}", label: "Document" },
  audio: { glyph: "\u{f001}", label: "Audio" },
  video: { glyph: "\u{f03d}", label: "Video" },
  code: { glyph: "\u{f121}", label: "Code" },
};

/** The filters Tab cycles through: a label, the kinds it keeps (none = everything), and how results are laid out. */
export const FILTERS = [
  { label: "All", kinds: [], grid: false },
  { label: "Photos", kinds: ["image", "screenshot"], grid: true },
  { label: "Documents", kinds: ["pdf", "doc"], grid: false },
  { label: "Media", kinds: ["audio", "video"], grid: false },
  { label: "Code", kinds: ["code"], grid: false },
];

/** @param {string} kind */
export function glyph(kind) {
  return (KINDS[kind] || KINDS.doc).glyph;
}

/** `1,204` */
export function count(n) {
  return String(Math.round(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** How many results a filter chip stands for, from the per-kind counts of the unfiltered list. */
export function filterCount(filter, counts) {
  if (!counts) return null;
  const kinds = filter.kinds.length ? filter.kinds : Object.keys(counts);
  return kinds.reduce((sum, k) => sum + (counts[k] || 0), 0);
}

/** The tag on a row: why it matched and where. */
export function tag(row) {
  const where = row.locatorLabel;
  switch (row.reason) {
    case "picture": return row.kind === "video" ? `seen at ${where}` : row.kind === "pdf" ? `page ${where.replace("p. ", "")} looks like it` : "by picture";
    case "sound": return where ? `said at ${where}` : "by sound";
    case "name": return "by name";
    case "words": return where && row.kind === "pdf" ? `words · ${where}` : "words";
    default: return where ? `text · ${where}` : "text";
  }
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** `2 min ago`, `today 14:05`, `yesterday`, `Mon`, `Mar 3`, `Mar 3, 2024`. */
export function when(seconds) {
  if (!seconds) return "";
  const then = new Date(seconds * 1000);
  const now = new Date();
  const ago = (now.getTime() - then.getTime()) / 1000;
  if (ago < 60) return "just now";
  if (ago < 3600) return `${Math.floor(ago / 60)} min ago`;
  const day = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((day(now) - day(then)) / 86400000);
  const hm = `${String(then.getHours()).padStart(2, "0")}:${String(then.getMinutes()).padStart(2, "0")}`;
  if (days === 0) return `today ${hm}`;
  if (days === 1) return "yesterday";
  if (days < 7) return ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][then.getDay()];
  const md = `${MONTHS[then.getMonth()]} ${then.getDate()}`;
  return then.getFullYear() === now.getFullYear() ? md : `${md}, ${then.getFullYear()}`;
}

/** `412 KB` */
export function bytes(n) {
  if (!n) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1000 && i < units.length - 1) {
    n /= 1000;
    i++;
  }
  return `${n >= 100 || i === 0 ? Math.round(n) : n.toFixed(1)} ${units[i]}`;
}

/** `about 1 h 50 min` */
export function duration(seconds) {
  if (seconds == null) return "";
  if (seconds < 90) return "under 2 min";
  const m = Math.round(seconds / 60);
  if (m < 60) return `about ${m} min`;
  return `about ${Math.floor(m / 60)} h ${m % 60} min`;
}

/**
 * `text` cut into pieces, each saying whether it is one of `words` (case-insensitive, as typed, half words too):
 * what a row or the preview paints in the accent.
 * @param {string} text @param {string[]} words @returns {{ text: string, hit: boolean }[]}
 */
export function marks(text, words) {
  const wanted = words.filter((w) => w.length >= 2).map((w) => w.toLowerCase());
  if (!text || wanted.length === 0) return [{ text, hit: false }];
  const lower = text.toLowerCase();
  const out = [];
  let at = 0;
  while (at < text.length) {
    let next = -1;
    let len = 0;
    for (const w of wanted) {
      const i = lower.indexOf(w, at);
      if (i !== -1 && (next === -1 || i < next || (i === next && w.length > len))) {
        next = i;
        len = w.length;
      }
    }
    if (next === -1) {
      out.push({ text: text.slice(at), hit: false });
      break;
    }
    if (next > at) out.push({ text: text.slice(at, next), hit: false });
    out.push({ text: text.slice(next, next + len), hit: true });
    at = next + len;
  }
  return out;
}

/** `#rrggbb` or `#rrggbbaa` at `alpha` (0..1). */
export function alpha(color, a) {
  const hex = String(color).replace("#", "").slice(0, 6);
  return `#${hex}${Math.round(Math.max(0, Math.min(1, a)) * 255).toString(16).padStart(2, "0")}`;
}

/** Markdown-safe text, for TextView. */
export function escapeMarkdown(text) {
  return text.replace(/([\\`*_{}\[\]()#+\-.!|>~<])/g, "\\$1");
}
