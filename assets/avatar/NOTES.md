# Production avatar contract

Status: **OWNER APPROVED FINAL**

Version: **final11**

Native size: **48×48 CSS px**

## What production may consume

- `avatar.json` — canonical runtime asset used by `src/is-tibo-happy.mjs`.
- `png/tibo-{happy,unhappy}-48.png` — native lossless visual references.
- `png/tibo-{happy,unhappy}-{96,144,192,384}.png` — exact integer nearest-neighbor exports.
- `MANIFEST.json` — version, aliases, export rules, and SHA-256 checksums.

There are two image variants only:

- `happy`: also used by WAITING.
- `unhappy`: also used by OFFLINE with the existing grayscale/opacity treatment.

## Rendering requirements

- Preferred path: render `avatar.json` to a 48×48 canvas and display at exactly 48 CSS px.
- Keep `image-rendering: pixelated`.
- If using a PNG, select an integer-size export and display at an exact divisor/multiple of 48.
- Apply rounded corners in CSS; symbols are already positioned inside the safe area.

Forbidden: JPEG, lossy WebP/AVIF, bilinear/bicubic resizing, smoothing filters, screenshots, contact-sheet crops, or historical files under `designs/avatars/archive/`.

Rebuild only from the approved JSON grids:

```sh
uv run --with pillow python designs/avatars/approved/build_assets.py
```
