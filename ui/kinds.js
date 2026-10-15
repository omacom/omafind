// What each kind of file looks like in a row: a Nerd Font glyph (Omarchy's font ships them) and a name.

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

/** The filters Tab cycles through: a label and the kinds it keeps (none = everything). */
export const FILTERS = [
  { label: "All", kinds: [] },
  { label: "Photos", kinds: ["image", "screenshot"] },
  { label: "Documents", kinds: ["pdf", "doc"] },
  { label: "Media", kinds: ["audio", "video"] },
];

/** @param {string} kind */
export function glyph(kind) {
  return (KINDS[kind] || KINDS.doc).glyph;
}

/** `1,204` */
export function count(n) {
  return String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}
