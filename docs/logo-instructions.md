# CRT logo SVG reconstruction

How to rebuild `docs/crt.png` as an SVG of large, purposeful paths.
This is a geometric approximation, not an auto-trace.

## Source

- Raster: `docs/crt.png`
- Size: 1448 x 1086, RGBA
- Opaque silhouette: about 1260 x 1047 at offset (95, 18)
- Suggested viewBox: `0 0 1448 1086` (or the opaque bbox once
  the drawing is stable)

The mark is a beige CRT television. Lighting is darker on the left
and lighter on the right. The screen is dark teal with hollow mint
neon lettering.

## Goal

Close visual match with a small set of distinct shapes. Every path
should have a clear job (housing, well ring, cream lip, gasket, a
letter stroke). No tiny irregular blobs, no per-scanline rects, no
outlined subtitle glyphs.

## Construction model

Do not tile adjacent mitered puzzle pieces.

For each object:

1. Draw one large **base** path in the mid or dominant colour.
2. Stack smaller **overlays** for highlights and shadows, clipped
   to that base.
3. Draw a new base only when the thing is a new object (gasket,
   glass, knob, slot), not when it is only a shade of an existing
   one.

Rules of thumb:

- **New object** → its own base path.
- **Light or shadow on that object** → overlay, clipped to the
  base.
- **Hard 3D face** (cream lip against a dark well) → opaque
  overlay. The overlay’s edge *is* the face boundary.
- **Soft plastic lighting** (housing left/right) → the overlay may
  itself be a gradient that dies into the base. It does not have
  to cover a full half; the middle of the object can stay the base
  colour.

## Paint order

Back to front:

1. Housing (base, lighting overlays, rim, outer border)
2. Inner well (dark ring, face overlays, black gasket)
3. Screen (glass, scanlines, glass highlights), clipped to the
   well opening
4. Neon `CRT` lettering and glow
5. Subtitle and rules
6. Control-deck details (panels, slot, vent, knobs)
7. Stand

## Colour notes

Sampled from the PNG; mix as gradients rather than using them as
flat fills everywhere.

| Role                    | Sample   |
|-------------------------|----------|
| Outer border, dark left | `#2C2626` |
| Outer border, light right | `#ECEED9` |
| Housing face, left      | `#A1A08B` |
| Housing face, right     | `#DADCBE` |
| Well ring (top/dark)    | `#050605` |
| Screen glass, dark      | `#0A1616` |
| Neon stroke             | `#EDFDF4` |
| Glass highlight         | `#6DC498` |
| Stand face              | `#433E39` |

Housing beige sits in a grey-green range, not a warm yellow.

---

## 1. Housing

The outer silhouette is **not** one rounded rect. The main body is;
a narrower **chin** steps in under the control strip (speaker,
cassette, vent).

### Base

- **Body:** large rounded rect. Includes the screen area and the
  control strip with the knobs.
- **Chin:** narrower rounded rect overlapping the bottom of the
  body.

Fill both with the same mid beige. They are one object (the
plastic), drawn as two rects so the silhouette stays easy to edit.

The concave step-in where body meets chin comes from that overlap,
not from extra corner pieces.

### Overlays (clipped to body ∪ chin)

- **Left shadow:** darker beige. A band or rounded shape on the
  left, not necessarily a full half.
- **Right/top highlight:** lighter beige on the lit side.
- **Rim highlight:** thin inset stroke, light, strongest on the
  top and right.

### Border

- **Outer ring / thick stroke** following the body+chin
  silhouette. Linear gradient dark grey on the left to light grey
  on the right.

A thin dark line where shapes meet the outer edge is this border,
not a separate path.

---

## 2. Inner well

The well is a 3D recess around the glass, seen slightly from
above. It is **not** one uniform ring of a single shade, and it is
**not** four mitered frame sides that have to join on diagonals.

There is no dark bottom cut-face: from this angle that face is
hidden, and you see the cream inner lip instead.

### Base

- **Dark rounded-rect ring.** Thick, large corner radii. Colour of
  the top wall (`#050605` range). This *is* the recess. The left
  wall can just be this base; it does not need its own overlay.

The inner edge of this ring is the screen opening. A rounded rect
with large radii is the first approximation; the PNG bows slightly
on all four sides.

### Overlays (clipped to the ring)

- **Right-wall band:** lighter grey on the right of the ring. The
  lit side of the recess.
- **Bottom cream U:** wide cream/beige band along the bottom
  curve, wrapping the bottom-left and bottom-right corners. The
  big lit inner lip. Thickest well band on the bottom.
- **TL cream L:** cream L wrapping the inner top-left corner,
  between the dark ring and the gasket. A real third face at that
  corner, and large enough to keep.
- **TR cream L:** the same on the inner top-right.

W4 / W5 / W6 are the same cream family as the housing, but they
are **not** one ring: the cream dies out along the mid-sides and
is absent along the mid-top. Three pieces.

### On top (new object)

- **Black gasket:** thick black rounded-rect ring just inside the
  well overlays. Uniform thickness. Its inner edge is the glass
  opening.

A hairline between gasket and glass is screen vignette, not a well
path. Do not split the dark ring into two greys; it reads as one
plane with a slight left-to-right gradient.

### Corner reading (outside → in)

Top-left: housing → dark ring → cream L → gasket → glass.

Bottom-left: housing → dark ring → cream U → gasket → glass.

---

## 3. Screen

Clip everything in this section to the gasket’s inner opening.

- **Glass fill:** same rounded shape as the opening. Dark teal,
  brighter toward the upper-right, darker toward the lower-left.
  A radial or linear gradient on this fill can do the vignette.
- **Scanlines:** an SVG pattern of two horizontal bands (darker /
  slightly lighter teal), not one path per line.
- **Glass highlight A:** large diagonal quad / parallelogram.
  Soft, translucent mint wash over the upper-right quadrant.
- **Glass highlight B:** smaller, more opaque rounded rect in the
  far top-right (`#6DC498` range).

---

## 4. Neon `CRT`

Hollow double-stroke lettering: thin mint-white strokes
(`#EDFDF4`) with round caps. The dark screen shows through the
gap. These are letter geometry, not shade overlays.

Glow is **one** `feGaussianBlur` on a duplicate group behind the
strokes, not extra paths. Overlap strokes at joins and let round
caps meet; no connector blobs.

### C

Two arcs, same centre, open on the right (about 240–270°):

- Outer arc
- Inner arc

### R

Same double-line idea, three parts:

- Stem: pair of verticals
- Bowl: pair of arcs (a D, open to the stem)
- Leg: pair of diagonals

### T

- Crossbar: one elongated rounded-rect **outline** (two
  horizontals joined at the round ends)
- Stem: pair of verticals that stop under the crossbar, open at
  the bottom, round caps

---

## 5. Subtitle

- **Left and right rules:** two horizontal strokes, round caps.
- **`CODE REVIEW TOOL`:** a `<text>` element (or one outlined
  group if a matching font is unavailable). Single-stroke neon,
  same mint and glow as `CRT`. **Not** hollow, **not** per-glyph
  ribbon paths.

---

## 6. Control deck

Simple rounded rects and circles on the chin and the lower body.

- **Left panel:** rounded rect (blank plate).
- **Cassette body:** rounded rect.
- **Cassette opening:** darker inset rounded rect (a hole, so a
  new base, not a shade). Optional thin highlight on the top edge
  of the body.
- **Centre recess:** long dark rounded rect.
- **Centre lip:** thin light stroke along the top of that recess.
- **Vent well:** rounded rect.
- **Vent slats:** three horizontal bars, not a handful of
  hairlines.

### Knobs

Four circles: small, medium, and large on the control strip;
one more on the chin, below the large knob.

Each knob:

- Base circle, near-black
- Small highlight ellipse on top

Two paths per knob. No extra rings unless a later pass on the
large knob needs them.

---

## 7. Stand

- **Foot silhouette:** wide squat rounded rect (a slight trapezoid
  is optional).
- **Top band:** darker overlay across the top.
- **Highlight stripe:** thin light band across the face.
- **Bottom band:** darker overlay at the ground contact.

---

## Not paths

- Per-scanline rectangles
- 3D chamfer triangles or diagonal miter pieces in the well
- Outlined subtitle glyphs
- Glow, grain, or phosphor texture
- Drop shadow (the PNG is the dark rim of the housing border on
  black)
- Tiny inner hairlines on the gasket

---

## Build order

Iterate and check the silhouette before adding type.

1. Housing base (body + chin), left/right lighting overlays, rim,
   outer border. Does it read as a TV?
2. Well ring, right-wall overlay, cream U, cream L’s, gasket. Does
   the recess sit in the body?
3. Glass fill, scanline pattern, two highlights.
4. `C`, then `R`, then `T`, then subtitle.
5. Deck details, knobs, stand.

## Reference raster

Keep `docs/crt.png` as the visual source of truth while drawing.
The SVG should match its large colour bands and letter construction,
not its pixel grain.
