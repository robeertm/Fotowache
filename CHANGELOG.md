# Changelog

All notable changes to Fotowache. Versions follow [Semantic Versioning](https://semver.org/).

## [2.0.0] - 2026-10-20

First public release.

### Rendering
- New processing pipeline ("process 2") in a linear wide-gamut working space
  (ROMM RGB) following the DNG specification: dual-illuminant camera profiles with forward matrices,
  hue/saturation maps and look tables, the DNG standard base tone curve,
  Kelvin/tint white balance with Bradford adaptation, gamut mapping on output.
- Local tone mapping for highlights, shadows, whites, blacks and clarity on an
  edge-aware base layer, tuned against finished reference edits (average
  difference about 0.3 EV on photos that were not used for tuning).
- Automatic settings per photo, learned from real finished edits.
- XMP sidecars are read: exposure, tone, white balance, crop and other basic
  settings become the starting point. Vibrance above +15 from XMP sidecars is
  scaled down, so portraits edited with high vibrance keep natural skin tones.
- Sharpening and texture work in sensor pixels, so the preview, 100 % zoom and
  the full-size export look the same.

### Cameras and lenses
- Every camera the built-in RAW decoder supports — more than a thousand
  models, from compact cameras to medium format, drones and phones that save
  DNG — checked on samples from more than 25 models.
- DCP camera profiles and LCP lens profiles that other software installed on
  the same computer are used; they are never bundled.
- Camera profiles embedded in DNG files are used when no installed profile
  exists, and can be chosen explicitly.
- Lens corrections from the camera file, DNG correction opcodes, installed
  lens profiles or the bundled open lens database, including chromatic
  aberration. Without a profile no correction is guessed.
- Reduced-size raws that are already demosaiced in the camera are
  white-balanced correctly.

### Editing
- Basic panel: profile, white balance in Kelvin with presets and an
  eyedropper, exposure in stops, tone and presence sliders.
- Clipping warning, one undo step per slider drag, copy and paste of settings,
  keyboard shortcuts for all frequent actions.
- English and German user interface.

### Packages
- Self-contained packages for all three major desktop systems (installer,
  disk image, AppImage and portable archives), with install commands that
  verify SHA-256 checksums.
- Falls back to the system's software renderer on computers without a
  supported graphics card.
