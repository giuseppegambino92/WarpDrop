// QR code drawn from the engine's module matrix as crisp SVG squares.
import { useEffect, useMemo, useState } from "react";
import { api } from "../lib/api";
import type { QrMatrix } from "../lib/types";

const QUIET = 4; // modules of quiet zone on each side

const cache = new Map<string, QrMatrix>();

function useQrMatrix(text: string): QrMatrix | null {
  const [m, setM] = useState<QrMatrix | null>(() => cache.get(text) ?? null);
  useEffect(() => {
    const hit = cache.get(text);
    if (hit) {
      setM(hit);
      return;
    }
    let alive = true;
    api
      .qrMatrix(text)
      .then((r) => {
        cache.set(text, r);
        if (alive) setM(r);
      })
      .catch(() => alive && setM(null));
    return () => {
      alive = false;
    };
  }, [text]);
  return m;
}

/** One SVG path: horizontal runs of dark modules merged into rectangles. */
function matrixPath(m: QrMatrix): string {
  let d = "";
  for (let y = 0; y < m.size; y++) {
    let x = 0;
    while (x < m.size) {
      if (!m.modules[y * m.size + x]) {
        x++;
        continue;
      }
      const start = x;
      while (x < m.size && m.modules[y * m.size + x]) x++;
      d += `M${start + QUIET} ${y + QUIET}h${x - start}v1h${start - x}z`;
    }
  }
  return d;
}

export function QrCode({ text, size = 200, label }: { text: string; size?: number; label: string }) {
  const m = useQrMatrix(text);
  const d = useMemo(() => (m ? matrixPath(m) : ""), [m]);
  const dim = (m?.size ?? 25) + QUIET * 2;
  return (
    <div
      className="shrink-0 overflow-hidden rounded-card bg-white hairline"
      style={{ width: size, height: size }}
      role="img"
      aria-label={label}
    >
      {m && (
        <svg width={size} height={size} viewBox={`0 0 ${dim} ${dim}`} shapeRendering="crispEdges">
          <rect width={dim} height={dim} fill="#ffffff" />
          <path d={d} fill="#111113" />
        </svg>
      )}
    </div>
  );
}
