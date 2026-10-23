# Resampling

Below 100% an output pixel covers more than one texel, and the only right
answer is the average of what it actually covers — weighted by the fraction of
each edge texel the pixel overlaps, so that the result does not shimmer as the
zoom changes. Above 100% there is no right answer, only a question about what
the image is for, so it is a setting:

| Filter | What it is for |
| --- | --- |
| `nearest` (default) | Nearest neighbor, ramped across the single output pixel that straddles a texel edge. Shows the pixel grid a measurement image is read on. At a whole-number zoom it is exactly nearest neighbor; at 4.5:1 it resolves the half-covered pixel rather than doubling columns unevenly, which is what plain nearest does. |
| `bicubic` | Catmull-Rom. Interpolating, so texel centers come through untouched, and noticeably sharper than bilinear. What you want when the subject is a photograph. |

At and above 1:1 the quad is put on whole pixels, so a texel edge falls on a
pixel edge and `nearest` is exactly nearest. Centering an odd difference
otherwise leaves it half a pixel off the grid, which would put every texel edge
through the middle of a pixel and cost the 100% view its crispness.

All three filters are weighted sums of texel loads in `shaders/image.wgsl`
rather than sampler taps — one bilinear tap is neither of the two above, and it
covers four texels however far out the view is zoomed. Because the shader does
the weighting, it can also multiply straight alpha through first, so a
transparent texel does not bleed its color into the edge beside it.

## What it costs, and the coarse chain

An exact area filter reads every source texel the window covers, which is a
constant — one pass over the visible image — whatever the zoom. That is
pleasant at 24 megapixels and gigabytes a frame at the texture size limit, and
with a wheel to zoom by it would be paid again on every notch.

So each image gets a chain of coarse levels, each an exact 4×4 area average of
the one above it. A draw starts from the level within a factor of four of the
size it wants, which bounds it at sixteen texels per output pixel however far
out the view is. Stepping by four per axis rather than a mipmap's two is what
makes that affordable: levels shrink by sixteen in area, so the whole chain is
a *fifteenth* of the image's own texture where a mip chain is a third of it.

It is built the first time a view zooms out past 4:1 — most never do — and it
is dropped with the image, so only one is ever alive. Levels are float, which
keeps them linear with no transfer function to think about and no 8-bit floor
under premultiplied color: half floats, or 32-bit ones above a 32-bit float
source, where the range and the low bits are the point of the file.

## No data

A raster with a no-data sentinel (`DecodedImage::nodata`, a GeoTIFF's
`GDAL_NODATA`) is mostly not picture: a map of drainage divides or streams is
one-pixel lines across a field of the sentinel. Averaged with the sentinel, a
line crossing a 4×4 block keeps a quarter of its value, and the window, which
the statistics set from the data alone, takes that to black. The line goes
from the screen at 25% without having gone from the file.

So the area filter leaves the sentinel out, as `image::resample` does for the
thumbnails: a pixel is the mean of the data under it, and where there is none
it is the sentinel, which the window draws as it draws the sentinel at full
size. To keep that exact across levels of the chain, each level carries the
share of its block that is data, where alpha would be, with the data
premultiplied by it — the arithmetic of premultiplied alpha, and the reason a
mean of means weighted by coverage is the mean of the data. A gray chain takes
a second component for the share (`Layout::level_format`), and a color one has
its fourth. `covered`, `with_coverage` and `resolved` in `shaders/texel.wgsl`
are the three steps; `shader_codes::coverage` says which a pass takes.

The sentinel is compared with each texel of the picture as uploaded, so
`upload::sentinel` works it out as that texture reads: through the same scale,
table or curve `upload::plan` stored the samples by, with half the gap to the
next code's value as the tolerance for an integer picture, whose bytes the
device decodes with a rounding of its own. A color texel is the sentinel only
in all three channels. A picture with an alpha channel has no sentinel here:
its coverage already says where the picture is, and its levels have no channel
left for another. Magnified, the filters read the texels as they are, so the
sentinel shows as the file holds it.


## The export's resize, on the CPU

An export at a size other than the picture's own is resampled by
`resample::resize`, not by the GPU: the write is on a copying thread that
holds no device, and it starts from the 8-bit sRGB raster
`encode::displayed` has already walked out, the display pipeline baked in.
It is the shaders' two filters again, in Rust, over that raster. Along an
axis the picture shrinks each output pixel is the area average of what it
covers, the edge source pixels weighted by their overlap; along one it
grows it is Catmull-Rom over the four source pixels around the output
pixel's center, clamped to the picture's edge, with `catmull_rom` copied
from the shader so a source pixel's center comes through untouched and the
undershoot past black is clamped as the shader clamps it. An axis that
keeps its length is an exact identity. Nearest is not offered: a file
enlarged by nearest is a file of blocks, and the dialog warns where the
screen is showing nearest and the export will not.

The arithmetic is in linear light on premultiplied color, as the shaders',
so the mean of black and white is the code of half the light, not the
middle code, and a transparent pixel does not darken the edge beside it.
The raster is decoded through the sRGB curve, filtered, divided back out
by its coverage and taken through `encode::quantize`, the same table the
walk wrote it with. Doing this to the displayed raster rather than to the
linear texels is the one place the export parts from the screen, which
resamples before the window and the curve; the two agree exactly at the
picture's own size, where the export bypasses the filter, and differ by
the curve's nonlinearity across a resampled edge otherwise.

The picture is never held as floats whole: a picture at the size ceiling
is a gigapixel, and four floats a pixel of it is sixteen gigabytes. Each
band of output rows keeps a short run of source rows, each decoded and
resampled across once, and lets a row go once no output row below reads
it — four rows deep for a growth, a span's worth for a shrink. The bands
are divided between threads by the same count `encode::displayed` divides
its walk by, and a test holds the divided resize to the plain one.
