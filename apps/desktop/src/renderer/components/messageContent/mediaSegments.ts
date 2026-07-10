const IMAGE_EXT_RE = /\.(png|jpe?g|gif|webp|svg|bmp|avif)$/i;
const FILE_EXTENSIONS =
  "png|jpe?g|gif|webp|svg|bmp|avif|pdf|txt|md|csv|json|docx?|xlsx?|pptx?|zip|tar|gz";

const MEDIA_RE = /MEDIA:[ \t]*(?:`([^`\n]+)`|"([^"\n]+)"|'([^'\n]+)'|(\S+))/g;
const BACKTICK = "`";
const CODE_OUTPUT_PATH_RE = new RegExp(
  String.raw`(?:^|[\n\r])([^\n\r]*?\b(?:file|path|saved(?:\s+(?:to|at))?|output|result|image)\s*:\s*)` +
    String.raw`(?:[*_]{1,2})?\s*` +
    BACKTICK +
    "([^`\\n\\r]+\\.(?:" +
    FILE_EXTENSIONS +
    "))" +
    BACKTICK,
  "gi"
);

export type MediaToken = {
  isImage: boolean;
  isUrl: boolean;
  name: string;
  src: string;
};

export type MediaSegment =
  | {
      start: number;
      type: "text";
      value: string;
    }
  | {
      raw: string;
      start: number;
      token: MediaToken;
      type: "media";
    };

type Hit = {
  end: number;
  raw: string;
  start: number;
  token: MediaToken;
};

function codeRanges(content: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  let match: RegExpExecArray | null;
  const fenced = /```[\s\S]*?```/g;
  while ((match = fenced.exec(content)) !== null) {
    ranges.push([match.index, match.index + match[0].length]);
  }
  const inline = /`[^`\n]+`/g;
  while ((match = inline.exec(content)) !== null) {
    ranges.push([match.index, match.index + match[0].length]);
  }
  return ranges;
}

function isInRange(index: number, ranges: Array<[number, number]>): boolean {
  return ranges.some(([start, end]) => index >= start && index < end);
}

function overlaps(start: number, end: number, hits: Hit[]): boolean {
  return hits.some((hit) => start < hit.end && end > hit.start);
}

function toToken(raw: string, wasQuoted: boolean): MediaToken | null {
  let src = raw.trim();
  if (!wasQuoted) {
    src = src.replace(/[).,;:!?\]}]+$/, "");
  }
  if (!src) return null;
  const isUrl = /^(?:https?:\/\/|data:image\/)/i.test(src);
  const name = /^data:image\//i.test(src)
    ? "Image preview"
    : src.split(/[\\/]/).filter(Boolean).pop() || src;
  return {
    isImage: /^data:image\//i.test(src) || IMAGE_EXT_RE.test(src),
    isUrl,
    name,
    src
  };
}

export function describeMediaSrc(src: string): MediaToken {
  return (
    toToken(src, true) ?? {
      isImage: false,
      isUrl: false,
      name: src,
      src
    }
  );
}

export function splitMediaSegments(content: string): MediaSegment[] {
  const code = codeRanges(content);
  const hits: Hit[] = [];
  let match: RegExpExecArray | null;

  MEDIA_RE.lastIndex = 0;
  while ((match = MEDIA_RE.exec(content)) !== null) {
    if (isInRange(match.index, code)) continue;
    const quoted = match[1] ?? match[2] ?? match[3];
    const token = toToken(quoted ?? match[4] ?? "", quoted !== undefined);
    if (!token) continue;
    hits.push({
      end: match.index + match[0].length,
      raw: match[0],
      start: match.index,
      token
    });
  }

  CODE_OUTPUT_PATH_RE.lastIndex = 0;
  while ((match = CODE_OUTPUT_PATH_RE.exec(content)) !== null) {
    const rawPath = match[2] ?? "";
    const raw = `\`${rawPath}\``;
    const relativeStart = match[0].lastIndexOf(raw);
    if (relativeStart < 0) continue;
    const start = match.index + relativeStart;
    const end = start + raw.length;
    if (overlaps(start, end, hits)) continue;
    const token = toToken(rawPath, true);
    if (!token) continue;
    hits.push({ end, raw, start, token });
  }

  hits.sort((a, b) => a.start - b.start);
  const segments: MediaSegment[] = [];
  let last = 0;
  for (const hit of hits) {
    if (hit.start > last) {
      segments.push({
        start: last,
        type: "text",
        value: content.slice(last, hit.start)
      });
    }
    segments.push({
      raw: hit.raw,
      start: hit.start,
      token: hit.token,
      type: "media"
    });
    last = hit.end;
  }
  if (last < content.length) {
    segments.push({
      start: last,
      type: "text",
      value: content.slice(last)
    });
  }
  return segments.length > 0 ? segments : [{ start: 0, type: "text", value: content }];
}
