You can inspect image metadata with `image_info`. Provide either:

- A base64-encoded image string (e.g. from a data URI or encoded file)
- A URL to an image (will be fetched automatically)

The tool returns:
- Image format (PNG, JPEG, GIF, WebP, BMP, TIFF)
- Width and height in pixels
- Color mode (RGB, RGBA, Grayscale, etc.)
- File size in bytes

It reads only the image headers, not the full image data. This is useful for quickly checking image properties without downloading or opening the file.
