# Assets

Shared visual assets for AXTON's README, documentation and example apps.

## Branding

- [Wordmark SVG](branding/axton-logo.svg) and [transparent PNG](branding/axton-logo.png).
- [Square icon SVG](branding/axton-icon.svg) and [PNG](branding/axton-icon.png).
- [Wordmark preview](branding/axton-logo-preview.png) on white.

The identity uses uppercase geometric lettering in near-black (`#111111`), with a ten-degree forward slant. The square icon uses the same A on white. There is no separate symbol, gradient, shadow or speed-line decoration. SVG masters contain paths and require no installed font.

Run `python3 assets/branding/generate.py` with Pillow installed to regenerate the SVG and PNG exports. The documentation site uses copies of the icon in `website/docs/assets/`; the two React Native examples use PNG copies in their `assets/` directories. Keep these copies synchronized after regeneration.

See [Marketing](../marketing/README.md) for content plans and production work.
