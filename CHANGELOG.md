# Changelog

All notable changes to Fotowache. Versions follow [Semantic Versioning](https://semver.org/).

## [2.1.0] - 2026-10-21

### Local AI
- New optional **local AI**, installed with one click (about 3.3 GB, pinned
  versions, SHA-256 verified, resumable). It runs entirely on the computer; the
  previous server-based AI is gone.
- **Develop with AI:** the AI describes and classifies the photo (scene, light,
  people, sky, haze, colour cast, suitable look); Fotowache turns that into
  slider changes on top of the automatic settings, checks brightness by
  rendering the result, and keeps all guard rails. Strength gentle / normal /
  strong, AI log with what was seen and changed.
- **AI fill** for the object removal brush: removes large objects by
  recreating the area from its surroundings.
- **Add:** paint an area and describe what should appear (e.g. clouds); the new
  content is placed into the raw data and follows all sliders.
- **Image generator** (bonus, separate download of about 5.2 GB): create images
  from a description, locally.
- Command line: `--ki-installieren`, `--ki-entfernen`, `--ki` (batch with AI),
  `--hinzufuegen`, `--bild`, `--generator-installieren`, `--ki-laufzeit-pruefen`,
  `--ohne-automatik` (all sliders at zero).

### Panorama
- New **Panorama…** for two or more selected photos, built on the published
  methods for this task: overlaps are found automatically in any order, photos
  that do not fit are left out with a reason, all photos are aligned together
  as rotations around one point with one shared focal length (bundle
  adjustment), the horizon is straightened, brightness is matched, seams are
  placed where the photos agree (moving people come from one photo only) and
  blended across several frequency bands.
- **Parallax correction:** where nearby and distant objects do not line up
  (handheld series), the photos are adjusted along the seams with a
  content-preserving mesh warp that keeps straight lines straight. If the
  photos were taken from clearly different positions, the report says so.
- Lens distortion, colour fringing and vignetting are corrected from the
  camera's or the database's lens profile before stitching; the panorama keeps
  the capture date.
- Projections cylinder, sphere and flat.
- Empty edges are either cropped to the largest fully covered rectangle (as an
  editable crop) or filled by the local AI.
- The panorama is saved as a linear DNG next to the photos and is developed
  like any other raw file.
- Command line: `--panorama <files…>` with `--projektion`, `--raender-fuellen`,
  `--ohne-zuschnitt`, `--ohne-parallaxe`, `--voll`.

### Updates
- Fotowache looks for a newer release at start and offers it with one button;
  the update is checked against the release checksums and installed in place
  (app bundle, setup, AppImage or command-line install), then Fotowache
  restarts. `FOTOWACHE_KEINE_UPDATES=1` turns the check off; command line
  `--update`.

### Fixes
- The status bar no longer shows a stray separator for files without a camera
  name.
- Very large images (panoramas, medium format above about 60 megapixels or
  wider than 16384 pixels) no longer crash the export: they are developed in
  tiles that share the whole image's tone base and noise level, so the result
  matches a single-piece development.
- Two developments of the same photo could differ in thin rows by a few
  levels; the development and output stages now run strictly one after the
  other.

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
