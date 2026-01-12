# SubCast

## Environment Variables

- `FPS` frame rate (default 25)
- `WIDTH` width (default 1920)
- `HEIGHT` height (default 1080)
- `BASELINE` baseline (default 1026)
- `FONT_PATH` font file path
- `FONT_SIZE` font size (default 60)
- `LINE_HEIGHT` line height multiplier (default 1)
- `SHADOW_ANGLE` shadow angle (default 45)
- `SHADOW_DISTANCE` shadow distance (default 0)
- `SHADOW_SIZE` shadow spread (default 0)
- `SHADOW_BLUR` shadow blur radius (default 0)
- `SHADOW_OPACITY` shadow opacity (default 1)

## Pipe

### Input

Each line must be formatted as `{startMS}\t{endMS}\t{line1}   {line2}   {lineN}` with optional line breaks represented by 3 consecutive spaces. Malformed lines will be skipped over.

Lines are processed sequentially without temporal overlaps. If the next line starts before the current line ends, it will not be rendered until the current line ends.

Input will not be read further when output is closed.

### Output

Stream of RGBA frames.