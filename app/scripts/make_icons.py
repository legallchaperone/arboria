"""Draw the app icon (no dependencies): an ink rounded square with a thin branching
twig; one tip, in indigo, is the idea being pursued. Writes
src-tauri/icons/{32x32,128x128,128x128@2x,icon}.png and icon.icns. Run from app/."""

import struct
import zlib
from pathlib import Path

INK = (26, 33, 30)        # --sage-12
SAGE = (215, 218, 217)    # --sage-6 (light)
INDIGO = (124, 150, 255)  # between --indigo-9 and -11
SS = 4                    # supersampling


def _bezier(p0, p1, p2, n=24):
    return [((1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t * t * p2[0],
             (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t * t * p2[1])
            for t in (k / n for k in range(n + 1))]


# A trunk from the bottom that forks twice; the right tip is the idea being pursued.
TRUNK = _bezier((0.50, 0.78), (0.50, 0.68), (0.50, 0.60))
LEFT = _bezier((0.50, 0.62), (0.50, 0.44), (0.34, 0.33))
RIGHT = _bezier((0.50, 0.62), (0.50, 0.42), (0.66, 0.29))
BRANCHES = [(TRUNK, 0.026), (LEFT, 0.026), (RIGHT, 0.026)]
TIP = (0.66, 0.29, 0.056)
BUDS = [(0.34, 0.33, 0.026)]


def _near(x, y, pts, w):
    for (ax, ay), (bx, by) in zip(pts, pts[1:]):
        dx, dy = bx - ax, by - ay
        d2 = dx * dx + dy * dy or 1e-12
        t = max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / d2))
        if (x - ax - t * dx) ** 2 + (y - ay - t * dy) ** 2 <= w * w:
            return True
    return False


def shape(x: float, y: float):
    """Colour at (x, y) in the unit square, or None (transparent)."""
    r = 0.22  # corner radius of the 0.9 tile (macOS-like)
    lo, hi = 0.05, 0.95
    if not (lo <= x <= hi and lo <= y <= hi):
        return None
    cx = min(max(x, lo + r), hi - r)
    cy = min(max(y, lo + r), hi - r)
    if (x - cx) ** 2 + (y - cy) ** 2 > r * r:
        return None
    tx, ty, tr = TIP
    if (x - tx) ** 2 + (y - ty) ** 2 <= tr * tr:
        return INDIGO
    for bx, by, br in BUDS:
        if (x - bx) ** 2 + (y - by) ** 2 <= br * br:
            return SAGE
    for pts, w in BRANCHES:
        if _near(x, y, pts, w):
            return SAGE
    return INK


def render(n: int) -> bytes:
    rows = []
    for j in range(n):
        row = bytearray([0])
        for i in range(n):
            acc = [0, 0, 0, 0]
            for sj in range(SS):
                for si in range(SS):
                    c = shape((i + (si + 0.5) / SS) / n, (j + (sj + 0.5) / SS) / n)
                    if c:
                        acc[0] += c[0]; acc[1] += c[1]; acc[2] += c[2]; acc[3] += 255
            k = SS * SS
            a = acc[3] // k
            if a:
                row += bytes([acc[0] * 255 // acc[3], acc[1] * 255 // acc[3], acc[2] * 255 // acc[3], a])
            else:
                row += bytes(4)
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main() -> None:
    out = Path("src-tauri/icons")
    out.mkdir(parents=True, exist_ok=True)
    pngs = {n: render(n) for n in (32, 128, 256, 512)}
    (out / "32x32.png").write_bytes(pngs[32])
    (out / "128x128.png").write_bytes(pngs[128])
    (out / "128x128@2x.png").write_bytes(pngs[256])
    (out / "icon.png").write_bytes(pngs[512])
    # icns: PNG payloads (ic07 128, ic08 256, ic09 512)
    body = b"".join(t + struct.pack(">I", len(pngs[n]) + 8) + pngs[n]
                    for t, n in ((b"ic07", 128), (b"ic08", 256), (b"ic09", 512)))
    (out / "icon.icns").write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


if __name__ == "__main__":
    main()
