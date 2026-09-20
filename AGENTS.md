# Repository agent rules

## Avatar source of truth

- The owner-approved avatar release is **final11**.
- Runtime code must read `assets/avatar/avatar.json`.
- Editable source grids live only in `designs/avatars/approved/`.
- Files under `designs/avatars/archive/` are historical evidence. Never import, copy, promote, compare as a candidate, or use them in product code unless the user explicitly asks for historical analysis.
- There are only two image variants: `happy` and `unhappy`. `waiting` reuses `happy`; `offline` reuses `unhappy` with the existing grayscale treatment.
- Do not regenerate the portraits from photos or contact sheets. Do not modify approved pixels without explicit owner approval.
- Run `uv run --with pillow python designs/avatars/approved/build_assets.py` after any explicitly approved source-grid change.
- PNG scaling must be lossless nearest-neighbor at integer multiples of 48. Never use JPEG, lossy WebP/AVIF, bilinear, bicubic, CSS blur, or a contact sheet as a source asset.

