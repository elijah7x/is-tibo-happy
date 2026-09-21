# Repository agent rules

## Avatar source of truth

- The owner-approved avatar release is **final11**.
- Runtime code must read `assets/avatar/avatar.json`; PNG renders live in `assets/avatar/png/`.
- There are only two image variants: `happy` and `unhappy`. `offline` reuses `unhappy` with the existing grayscale treatment.
- Do not modify approved pixels without explicit owner approval.
- PNG scaling must be lossless nearest-neighbor at integer multiples of 48. Never use JPEG, lossy WebP/AVIF, bilinear, bicubic, or CSS blur.
