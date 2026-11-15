#!/usr/bin/env python3
"""
Generate `servers/fb_server/src/wallpaper_640x348_rgb565.bin` from `assets/wallpaper_exo.jpg`
(or an explicit `--input <image>` path).

Center-crops cleanly to match the `1920x1044` ExoCompositor desktop canvas above the
36px panel, resizes with separable Lanczos-3 to `640x348` (exact 3:1 upscale
ratio), and encodes little-endian RGB565 (445,440 bytes) with 4x4 Bayer ordered dithering
to eliminate dark-gradient contour banding.
"""

import argparse
import math
import os
import struct
import subprocess
import tempfile
import zlib
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parent.parent
DEFAULT_SRC = ROOT_DIR / "assets/wallpaper_exo.jpg"
OUT_BIN = ROOT_DIR / "servers/fb_server/src/wallpaper_640x348_rgb565.bin"

DST_W = 640
DST_H = 348

BAYER_4X4 = (
    (0, 8, 2, 10),
    (12, 4, 14, 6),
    (3, 11, 1, 9),
    (15, 7, 13, 5),
)


def load_ppm_rgb(ppm_path: str):
    with open(ppm_path, "rb") as f:
        magic = f.readline().decode().strip()
        if magic != "P6":
            raise ValueError(f"Expected P6 PPM, got {magic!r}")
        line = f.readline().decode().strip()
        while line.startswith("#"):
            line = f.readline().decode().strip()
        w, h = map(int, line.split())
        maxval = int(f.readline().decode().strip())
        if maxval != 255:
            raise ValueError(f"Unsupported maxval={maxval}")
        raw = f.read(w * h * 3)
    return w, h, raw


def decode_image_to_rgb(src_path: str):
    try:
        from PIL import Image  # type: ignore

        img = Image.open(src_path).convert("RGB")
        w, h = img.size
        return w, h, bytearray(img.tobytes())
    except ImportError:
        pass

    with tempfile.NamedTemporaryFile(suffix=".ppm", delete=False) as tmp:
        ppm_path = tmp.name

    try:
        win_src = subprocess.check_output(["wslpath", "-w", src_path], text=True).strip()
        win_ppm = subprocess.check_output(["wslpath", "-w", ppm_path], text=True).strip()
        ps = (
            "Add-Type -AssemblyName System.Drawing; "
            f"$bmp = [System.Drawing.Bitmap]::FromFile('{win_src}'); "
            "$w = $bmp.Width; $h = $bmp.Height; "
            "$rect = New-Object System.Drawing.Rectangle(0,0,$w,$h); "
            "$bd = $bmp.LockBits($rect,[System.Drawing.Imaging.ImageLockMode]::ReadOnly,"
            "[System.Drawing.Imaging.PixelFormat]::Format24bppRgb); "
            "$stride = [Math]::Abs($bd.Stride); $raw = New-Object byte[] ($stride * $h); "
            "[System.Runtime.InteropServices.Marshal]::Copy($bd.Scan0,$raw,0,$raw.Length); "
            "$bmp.UnlockBits($bd); $bmp.Dispose(); "
            "$out = New-Object byte[] ($w * $h * 3); "
            "for($y=0;$y -lt $h;$y++){$s=$y*$stride;$d=$y*$w*3;"
            "for($x=0;$x -lt $w;$x++){$si=$s+$x*3;$di=$d+$x*3;"
            "$out[$di]=$raw[$si+2];$out[$di+1]=$raw[$si+1];$out[$di+2]=$raw[$si];}} "
            "$hdr=[System.Text.Encoding]::ASCII.GetBytes(\"P6`n$w $h`n255`n\"); "
            f"$fs=[System.IO.File]::Create('{win_ppm}'); "
            "$fs.Write($hdr,0,$hdr.Length); $fs.Write($out,0,$out.Length); $fs.Close();"
        )
        subprocess.run(["powershell.exe", "-NoProfile", "-Command", ps], check=True)
        w, h, raw = load_ppm_rgb(ppm_path)
        return w, h, bytearray(raw)
    finally:
        if os.path.exists(ppm_path):
            os.remove(ppm_path)


def crop_to_aspect(rgb: bytearray, sw: int, sh: int, target_w: int, target_h: int):
    target_ratio = target_w / target_h
    src_ratio = sw / sh
    if abs(src_ratio - target_ratio) < 1e-4:
        return sw, sh, rgb

    if src_ratio > target_ratio:
        crop_h = sh
        crop_w = max(1, min(sw, int(round(sh * target_ratio))))
    else:
        crop_w = sw
        crop_h = max(1, min(sh, int(round(sw / target_ratio))))

    x0 = (sw - crop_w) // 2
    y0 = (sh - crop_h) // 2
    out = bytearray(crop_w * crop_h * 3)
    row_bytes = crop_w * 3
    for y in range(crop_h):
        si = ((y0 + y) * sw + x0) * 3
        di = y * row_bytes
        out[di : di + row_bytes] = rgb[si : si + row_bytes]
    return crop_w, crop_h, out


def lanczos_kernel(x: float, a: float = 3.0) -> float:
    if abs(x) < 1e-8:
        return 1.0
    if abs(x) >= a:
        return 0.0
    px = math.pi * x
    return (math.sin(px) / px) * (math.sin(px / a) / (px / a))


def resize_lanczos(src: bytearray, sw: int, sh: int, dw: int, dh: int) -> bytearray:
    a = 3.0
    scale_x = sw / dw
    support_x = a * max(1.0, scale_x)
    inv_scale_x = 1.0 / max(1.0, scale_x)

    h_weights = []
    for dx in range(dw):
        center = (dx + 0.5) * scale_x - 0.5
        x0 = max(0, int(math.floor(center - support_x)))
        x1 = min(sw - 1, int(math.ceil(center + support_x)))
        ws = []
        wsum = 0.0
        for sx in range(x0, x1 + 1):
            w = lanczos_kernel((sx - center) * inv_scale_x, a)
            if w != 0.0:
                ws.append((sx, w))
                wsum += w
        h_weights.append([(sx, w / wsum) for sx, w in ws])

    inter = [0.0] * (sh * dw * 3)
    for y in range(sh):
        srow = y * sw * 3
        drow = y * dw * 3
        for dx, ws in enumerate(h_weights):
            r = g = b = 0.0
            for sx, w in ws:
                si = srow + sx * 3
                r += src[si] * w
                g += src[si + 1] * w
                b += src[si + 2] * w
            di = drow + dx * 3
            inter[di] = r
            inter[di + 1] = g
            inter[di + 2] = b

    scale_y = sh / dh
    support_y = a * max(1.0, scale_y)
    inv_scale_y = 1.0 / max(1.0, scale_y)

    v_weights = []
    for dy in range(dh):
        center = (dy + 0.5) * scale_y - 0.5
        y0 = max(0, int(math.floor(center - support_y)))
        y1 = min(sh - 1, int(math.ceil(center + support_y)))
        ws = []
        wsum = 0.0
        for sy in range(y0, y1 + 1):
            w = lanczos_kernel((sy - center) * inv_scale_y, a)
            if w != 0.0:
                ws.append((sy, w))
                wsum += w
        v_weights.append([(sy, w / wsum) for sy, w in ws])

    out = bytearray(dw * dh * 3)
    for dy, ws in enumerate(v_weights):
        drow = dy * dw * 3
        for dx in range(dw):
            r = g = b = 0.0
            dx3 = dx * 3
            for sy, w in ws:
                si = sy * dw * 3 + dx3
                r += inter[si] * w
                g += inter[si + 1] * w
                b += inter[si + 2] * w
            di = drow + dx3
            out[di] = max(0, min(255, int(round(r))))
            out[di + 1] = max(0, min(255, int(round(g))))
            out[di + 2] = max(0, min(255, int(round(b))))
    return out


def encode_rgb565_dithered(rgb: bytearray, w: int, h: int) -> bytes:
    """Encode RGB888 to little-endian RGB565 with unbiased 4x4 Bayer ordered dithering."""
    out = bytearray(w * h * 2)
    step5 = 255.0 / 31.0
    step6 = 255.0 / 63.0
    for y in range(h):
        brow = BAYER_4X4[y & 3]
        row_off = y * w
        for x in range(w):
            i = row_off + x
            t = (brow[x & 3] + 0.5) / 16.0 - 0.5
            r5 = max(0, min(31, int(round(rgb[i * 3] / step5 + t))))
            g6 = max(0, min(63, int(round(rgb[i * 3 + 1] / step6 + t))))
            b5 = max(0, min(31, int(round(rgb[i * 3 + 2] / step5 + t))))
            px = (r5 << 11) | (g6 << 5) | b5
            struct.pack_into("<H", out, i * 2, px)
    return bytes(out)


def decode_rgb565(rgb565: bytes, w: int, h: int) -> bytes:
    """Exact software mirror of `desktop.rs::decode_rgb565_idx` for visual preview."""
    out = bytearray(w * h * 3)
    for i in range(w * h):
        (v,) = struct.unpack_from("<H", rgb565, i * 2)
        r5 = (v >> 11) & 0x1F
        g6 = (v >> 5) & 0x3F
        b5 = v & 0x1F
        out[i * 3] = (r5 << 3) | (r5 >> 2)
        out[i * 3 + 1] = (g6 << 2) | (g6 >> 4)
        out[i * 3 + 2] = (b5 << 3) | (b5 >> 2)
    return bytes(out)


def write_png(rgb: bytes, w: int, h: int, path: Path):
    stride = w * 3
    scanlines = bytearray()
    for y in range(h):
        scanlines.append(0)
        scanlines.extend(rgb[y * stride : (y + 1) * stride])
    compressed = zlib.compress(bytes(scanlines), 9)

    def chunk(tag: bytes, data: bytes) -> bytes:
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", compressed) + chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def main():
    parser = argparse.ArgumentParser(description="Generate Exo-OS embedded RGB565 wallpaper.")
    parser.add_argument("--input", default=str(DEFAULT_SRC), help="Source wallpaper image path.")
    parser.add_argument("--preview-dir", default=None, help="Optional directory to write PNG previews.")
    args = parser.parse_args()

    sw, sh, raw = decode_image_to_rgb(args.input)
    cw, ch, cropped = crop_to_aspect(raw, sw, sh, DST_W, DST_H)
    resized = resize_lanczos(cropped, cw, ch, DST_W, DST_H)
    rgb565 = encode_rgb565_dithered(resized, DST_W, DST_H)

    expected = DST_W * DST_H * 2
    if len(rgb565) != expected:
        raise AssertionError(f"Expected {expected} bytes, got {len(rgb565)}")

    OUT_BIN.parent.mkdir(parents=True, exist_ok=True)
    OUT_BIN.write_bytes(rgb565)
    print(f"Wrote {OUT_BIN} ({len(rgb565)} bytes, {DST_W}x{DST_H} RGB565 from {sw}x{sh} -> crop {cw}x{ch})")

    if args.preview_dir:
        preview_dir = Path(args.preview_dir)
        write_png(bytes(resized), DST_W, DST_H, preview_dir / f"wallpaper_preview_{DST_W}x{DST_H}.png")
        decoded = decode_rgb565(rgb565, DST_W, DST_H)
        write_png(decoded, DST_W, DST_H, preview_dir / f"wallpaper_rgb565_{DST_W}x{DST_H}.png")
        print(f"Wrote previews in {preview_dir}")


if __name__ == "__main__":
    main()
