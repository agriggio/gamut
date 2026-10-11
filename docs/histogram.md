# The histogram panel

`src/ui/histogram/` draws the panel; `src/image/stats.rs` counts what it
plots; `src/image/display/` holds what its controls set. This page is about
the shape of the panel — what is on it, what is not, and why — rather than
about the plot's arithmetic, which the code explains beside itself. What the
controls do to the picture is [color management](color.md)'s page.

## Where the line is

`gamut` is a viewer. The question the panel exists to answer is *what does
this file hold* — is the shadow empty or merely dark, is the highlight gone
or merely bright, where in the file's range do its values actually sit — and
a control earns its place on the panel by helping answer it. Exposure does:
pushing a picture two stops up is how you find out whether a shadow has
anything in it. A black point and a white point do: they are how linear or
16-bit data becomes a picture at all. White balance, saturation and the like
do not. They answer no question about the file, they only make a different
picture, and a viewer that makes a different picture it cannot save is a tease
that ends in sidecar files. That work belongs to the programs on the open
menu. The panel is an instrument, not `Display`'s fields laid out as rows.

## A column of stages

The panel is the picture's way from the file to the screen, a section to a
stage, read from the screen back: **Output**, what goes out; **Display**,
what the display starts from and what it does to it; **Gain map**, how far
the map lifts the picture, for a picture that carries one; **Image**, the
picture as the file stores it — named for the picture rather than the file,
since the gain map is the file's too. `histogram::Section` is that list, in that
order, and `section::layout` lays out the ones the picture passes through
(`Section::applies`).

Output is at the top because what goes out is what the panel is most often
opened to ask about — is anything clipped, and how much — and the head of
the column is the part that is always in view. Below it the order is the
data's, so the column reads as one path rather than as four histograms that
happen to share a panel: a picture's red that is past white on the Display
plot is at the top of the Image plot under it, and the matrix that moved it
is the gap between the two. A layout that kept every section in one place
for every file would have to leave a hole where a picture has no gain map;
the column closes up instead, and the sections above the gap do not move.
The same holds while the gain map is shown in the picture's place: the Image
section is left out (`Section::applies`), since what the display then starts
from is the map in stops, which the Gain map section already plots — on the
range its metadata describes, in whole stops, each code spread over the
stretch it stands for — and an Image section would plot it again as a bare
measurement, on its measured ends, and under a name that says the file
stores it, which it does not. The column is then the flow it is on screen:
the map, the display, the output.
While the depth map is shown, the Gain map section is left out in its turn:
nothing on screen is lifted, and its header's share of the lift shown would
not be true of anything in the window. The Image section is then the depth
map's own samples. The switch to the gain map is still in the bar and on its
key, so leaving the section out strands nothing.

Every section is a fixed height (`Section::height`), and the column is as
tall as its sections, whatever the window. The two exceptions are both the
Display section's: its row of false colors, which only a gray image has, and
its row of windows, which only an image read as data has
(`section::is_data`, `display::rows_height`). Each makes the section taller
rather than coming out of the plot, so that the Display plot stands as tall
as the Output's on every file — the two plots read closely, side by side in
the eye, at one height — and on a gray file, or a measurement, the sections
below it move down by the row. On a window shorter
than that the side panel scrolls — an egui `ScrollArea`, the wheel's alone:
`DragScroll::Never`, since a drag in the column is a drag of a handle, the
band or the exposure's slider, and must never move the column instead.
Nothing is shrunk to fit and nothing is dropped. What the panel needs room
for at least is the head section (`histogram::MIN_HEIGHT`), which is what
`ui::PANELS_ROOM` and `ui::room` are measured against, so that the toggle is
alive wherever the question the panel is opened for can be answered; the
rest is a scroll away. The width is the [side panel's](interface.md#the-side-panel).

The panel is laid out as the [information panel](interface.md#the-side-panel)
is, so that the two read as two tabs of one panel: a row of buttons at its
head, as tall as the information's header (`info::CHIP_HEIGHT`), so the
hairline under it (`info::HEADER_GAP`) stays put as the tabs are switched —
the pair that reads the file as a photo or as data first (`Control::Reading`,
see [color](color.md#display-referred-scene-referred-and-measured)), since
it is upstream of everything in the column, and at the far end the reset,
with the clipped pixels' toggle just before it;
then the column, in a scroll area with its bar down the inner edge and the
gutter kept clear for it; and the sections parted by hairlines through the
gap between them (`info::SECTION_GAP`). Each section opens with a header
as the information's sections do: its mark and name in the accent
(`Section::mark`) — a monitor for Output, sliders for Display, the sun for
the gain map, as it wears everywhere else in the window, and a picture for
Image, as the information's Image section wears — and on the right what the pointer is reading at its
stage. The header's last square (`section::button`) is the section's own
button where it has one: the Gain map section's eye, which shows the map
in the picture's place, lit while it is. A section's readout ends short of
that square where the section has a button, and at the line's end where it
does not; a section given one later — the switch that passes a stage by —
takes the square the same way.
A stage added later — the development stages a ProRAW file describes, a
baseline exposure, a profile's tone curve, a gain table — is a `Section`
variant placed between Display and Image at a height of its own: a 1D stage
composes into the Display curve and takes a header of its own, and a stage
that lifts each pixel by its own amount is shaped as the Gain map section
is, a histogram of the lift with how much of it is applied in its header.

The row of buttons — the marks on the picture, and at the far end the
reset (`histogram::toolbar`) — acts on every plot in the column, or on the
picture, and on no one section, so it stands above all of them and stays
in view however far the column is scrolled. With nothing beside them, the
plots fill the column's width, which is a bin to the point:
`histogram::COLUMN_WIDTH` is the plot and the inset its ground is drawn
into, and the side panel's width is worked out from it.

## What each section plots

**Image** is `Face::stored`: `Stats::scan_as_stored`, the picture read with
the curve resolved and nothing else — no lift, and the file's own primaries
(`Reader::as_stored`). It is measured once, when the image arrives, on the
loader's or the player's thread beside the scan the display starts from; a
file whose reading moves no color — gray, or BT.709 with no gain map
(`DecodedImage::color_moves`) — is not walked twice, and the stats are
shared.

**Gain map** is `GainMap::lift_plot`: the map's histogram in stops at the
whole of the lift, evenly spaced, over the range the file describes the map
for — its lowest code to its highest (`GainMap::described_range`) — for the
reason a graded file's plot is over 0..1: the axis is the file's, and what
of it the picture uses is the plot's shape. A map described down into
darkening it never does leaves the stretch below its lowest code empty,
with 0 marked inside the axis, which is what the file says. The header
writes the range the picture's map does hold (`GainMap::lift_range`). The map is 8-bit, and its codes reach
stops through a curve wherever its gamma is not one or it is Apple's, read
through Rec. 709's: a bin to a code would put the axis on that curve, and a
code dropped into the one bin its middle falls in would comb. So each code
stands for the stretch of stops from halfway to the code below to halfway to
the code above, and its count is spread over the bins that stretch covers,
in proportion — the honest plot of a quantized value on an axis that is not
its own — counted in sixteenths of a pixel (`SHARES`) so that a code spread
thin keeps something in each bin. A map with a channel each has a plane
each, and its luminance plane is the mean of the three, spread pixel by
pixel over the mean of their stretches; a pixel stands on it at the mean of
`GainMap::stops_at`. How much of the
lift is applied (`GainMap::applied_stops`, the same number the information
panel's *Applied* row says) is written in the header while the pointer is
reading nothing — none on a screen with no room above white, which is most
screens. There is no band under the plot as the Output and Display plots
have: theirs show what a value comes out as, and one filled to the applied
share would only say what the header's words do, and on an SDR screen would
be an empty strip. The map's
histogram is a walk over the map, kept in egui's data for as long as the
picture's map is the same one. The section reads the picture's map
(`Current::picture_face`), so that it stays while the map is shown in the
picture's place, and its button is that switch.

**Display** is `Face::stats`, what the display starts from: lifted through
the gain map at the weight in force, in the working space's primaries — see
[color management](color.md). It is the plot the window, the exposure and
the curve act on, so the response curve is drawn over it always, and the
band and the rows under it. Its planes are drawn as every other section's
are, so that it compares with the Output's bin for bin; the curve stands
off them on an outline in the plot's ground (`CURVE_OUTLINE`), which keeps
it readable where it crosses the planes' lines.

### The axis

What a position on the Display and Image plots stands for is
`stats::Axis`, and everything that puts a value on either plot or reads one
off it — the scan, the band and its handles, the response curve, the rows
of stops, the pointer's rule, the handles' keys (`Display::step_black`) —
goes through `Axis::place` and `Axis::value`, so that the plot and what is
drawn over it cannot be spaced two ways. A file on a curve is plotted on
that curve, and a linear integer file — sensor counts, a measurement — on
its linear values; the reasons are with `stats::Plot`.

Light stored linear as floats — a Radiance picture, an EXR — is plotted in
stops instead (`Axis::Stops`): a value stands at its base-2 logarithm. Such
a file has no curve of its own and no top. A Radiance picture runs from
nothing to a few hundred with almost all of it in the bottom hundredth,
which a linear axis puts in its first bin and its first two *fine* bins:
the Output plot, rebinned from those, came out as two flat steps, and the
window, the handles and the curve were a sliver at the plot's left edge
that the exposure slider visibly did nothing to. In stops the shadows get
the room the eye gives them, a stop of exposure is a shift along the axis
by one stop, and the window is a stretch of the plot. A measurement keeps
its linear axis: its values are quantities rather than light, may be
negative, and have no stops anyone counts.

The axis's range is read off the picture (`stats::stops_axis`): the top is
the first whole stop at or above the brightest plotted channel, and the
floor the darkest lit stop after the bottom `FLOOR_TRIM` of the lit pixels
is left off — a thousandth, enough to keep a few stray pixels at next to
nothing from putting the floor tens of stops down and the picture back in
a sliver, and no more, since what is trimmed piles into the first bin — no
further than `STOPS_REACH` under the top, and at least a stop under it so
that the axis always has a span. A picture with no light in it has no top
for a stops axis and is plotted on the linear one, which is why the axis
is read from `Stats::axis` rather than worked out from the image again.

A value with no light in it — zero, or less — has no logarithm, and stands
at minus infinity, below every floor. A metered file's window opens at
0..1, so its black handle stands at the band's left end, hollow, as a
handle out past the band does; a drag of the band slides the white alone,
since no light moved along by any distance is still no light
(`track`); and the black point's key steps it up from the plot's floor,
where the handle is drawn (`Display::placed`). The response curve is drawn
on the same axis both ways, so a display doing nothing is still the
diagonal and a stop of exposure lifts the whole line by a stop; black is
the plot's floor (`plot::Scale`), which a response at black is drawn at.

**Output** is the Display plot pushed through the display's response and
encoded on sRGB's curve, over 0 to the surface's room above white
(`FrameInput::room`, which is `App::display_headroom`). Where a gray image
is read through a ramp — a false color, or the gray ramp a measurement is
spread along (`Display::ramp`) — its axis is the ramp instead, from one end
to the other with no room past it, and the band under the plot is the ramp
itself: the response is where along the ramp a value lands, and for the
gray ramp that is the screen's code, so the plot is the Display plot across
the window with what lies outside piled at the ends, which is the truth of
what the screen receives. Spaced on sRGB's curve it would put a value's bar
where the *light* of that code falls, a place the ramp never gave it. It is
rebinned, not scanned: `Stats::scan` counts the pixels into 4081 fine bins on the input's
axis (`stats::Fine`), and `Fine::rebinned` pushes each fine bin's value
through the response and spreads its count over the output bins it stands
for, out-of-range piling into the end it went out of. A handle dragged
changes the response on every frame, and four thousand bins through it is
nothing where two million pixels would be a frame each. The Display and
Image plots are the same fine bins rebinned through nothing, so all three
are binned alike.

### Why the counts are spread

Dropping each fine bin's count whole into the one output bin its value
lands in makes a comb on a plot of data with none in it: two grids beat
against each other. Fine bins eight and a half to an output bin put eight
and nine into neighbors by turns, a ripple of a tenth on a smooth picture;
and an 8-bit file's codes, on the axis a wide-gamut file's color is carried
onto — about −0.4 to 1.04 for a P3 photograph — stand 0.7 of a bin apart and
put one and two into neighbors by turns, a ripple of a third on a plot of
every code there is. Neither says anything about the picture.

So a fine bin's count is spread evenly over the stretch it stands for once
the response has had it. A quantized file's code (`Fine::code`, the step
between codes) stands for the stretch from halfway to the code below to
halfway to the code above, and codes spread so tile the axis and come out
even. A code is never spread wider than an output bin, though: where the
display pulls codes further apart than that — a stop or two of exposure on
an 8-bit file — each is spread over a bin's width around where it lands and
the bins between are left as empty as the screen leaves them. That comb is
the posterization a histogram is looked at for, and it stays. A float file
has no step, and each fine bin is spread over a bin's width. The counts are
kept in sixteenths of a pixel (`stats::SHARES`), so that a pixel spread over
two or three bins keeps a share in each; a plot is read by its bins against
one another, which the unit leaves alone.

There are 255 × 16 + 1 fine bins so that every output bin's middle is a fine
bin's, and an 8-bit file's codes on an axis of 0..1 stand each on the middle
of its own bin: a display that does nothing puts every one wholly back in
it, and a spike that is one code is one bin.

The step is the file's as stored. A gain map's lift pulls the codes further
apart than that, so a lifted plot can keep a little of the comb.

The color planes go through the same scalar response as the luminance, a
channel at a time. That is exact where the display treats each channel
alike — a window, an exposure, a clip — and an approximation under the
neutral curve, whose desaturation moves a channel by what the others are:
the planes are then plotted where each channel would go were it gray. They
are read for their shape, and the luminance is still exact.

The rebinned plot is kept in egui's data (`output::binned`), keyed on the
fine bins' address — the `Arc` itself is held beside it, so the address
cannot be another's — the window's offset and gain, the curve, whether a
false color is on, the surface, and its room. Where the room is not known —
an HDR surface whose monitor has not said — it is resolved as the largest
output any value reaches, rounded up to a whole stop above white, and the
key carries what it was resolved to.

## The row of stops

Under the Output and Display bands, and under the Gain map plot, a row of
whole stops (`plot::ev_scale`): a tick at each stop where its value falls on
the plot's axis, and its number under the ticks there is room to write.
Each section says where a stop is (`place`), and the ticks are worked out
from the value, not the bins, so they are exact wherever they are drawn.

- **Output** counts from white: its values are ratios to white already, so a
  stop is `2^n` encoded on sRGB's curve over the axis's top. On a surface with
  room above white the row runs past 0 to the room, which is the one thing
  the band, held to what the panel can show, cannot say. It is left off an
  axis that is a ramp, whose places are not light, and off a measurement
  (`Referred::Measured`) as the Display row is.
- **Display** counts from the value the window puts at white — the white
  handle, `Display::displayed_bounds` — so the plot reads as stops below
  clipping, as a camera's does, and a stop on the exposure slider moves the
  row by exactly a stop. A window whose white is past the plot's end has no 0
  on it, which is the truth: nothing plotted reaches the clip. It is left off
  a measurement (`Referred::Measured`), whose values are not light.
- **Gain map** counts the lift itself, the axis's own unit, so its ticks are
  evenly spaced.
- **Image** has one only on an axis in stops (`stored::stop_across`),
  counting from the file's own 1, which holds still while the window and
  the exposure move the Display row's 0: the values at the axis's ends,
  which the section writes on any other float axis, would be a column of
  zeros at three places across twenty stops. On every other axis its point
  is the file's own units, and its stops would be the Display row's again
  with no window on them.

Every row is worked out from 0 outward each way, so 0 is always written and
the numbers further out give way to those nearer 0. A side stops where its
ticks come closer than `EV_TICK_GAP`: on a curve the shadows crowd past
reading around eight stops under white, and on a linear axis, where each
stop down is half the distance of the last, after four or five. On an axis
in stops the ticks are evenly spaced and the row runs the plot's width.

## The pixel traced down the column

`histogram::marked` hands every section its `Mark`: the bin its rule stands
on and the words its header writes. A pointer on a pixel of the picture
marks every section at once, each with the pixel as its own stage has it —
the Image section reads `Current::sample_as_stored`, the Gain map section
reads `GainMap::stops_at`,
the Display section reads `Current::sample` and writes the value and what
the display makes of it, and the Output section writes that response held
to the surface's room. The rules down the column are one pixel's path from
the file to the screen.

The Image section marks the pixel a channel at a time (`Mark::channels`):
red, green and blue as the file holds them, and the luminance worked out
from them as the plot's luminance plane is, each with a rule on its own
plane in that plane's ink (`Plane::ink`) — and its number on a line of its own under the header, after a
dot in the same ink, a cell to a channel so the numbers stay put as the
pointer moves. Four numbers do not fit beside a heading at a float's
decimals, which is why the line is its own. They are written as the bottom
bar's Decimal format writes a pixel (`pixel::component`): counts for 8- and
16-bit samples, the number for floats, and the luminance put back on the
file's curve and scale so that it reads in the same units as the channels
beside it.
A file's channels land on its plot as its codes do, so each rule stands on
the bar its channel was counted in.

A pointer on one section's plot is reading that plot's axis and is on no
pixel besides, so only that section marks anything, with the value its bin
stands for. Every hit test is against the rectangles the pass laid out,
which the scroll has already moved, and a pointer over a section scrolled
out of the panel is over nothing (the scroll area's clip rect). A value off
a plot's axis is still written and marks no bar: the rule would otherwise
claim a bar that is not the pixel's.

## The Output section

The band under the plot is what each output comes out as, and the panel
cannot show past white: its target is SDR, so the stretch an HDR surface
lifts above white would be drawn at white, the same as white itself. That
stretch is overlaid with chevrons pointing right, in the accent
(`plot::Past::Chevrons`), so that it reads as lifted rather than as a run of
white. The Display band keeps an accent rule along its top instead
(`Past::Rule`): there the band is the levels track, its handles are drawn
over it, and a pattern would fight them.

### The share clipped at either end

`Plot::clipped`, asked of the Output plot at black and at white, is the
share of the samples the screen puts out at or below black and at or above
white, per plane, the worst plane reported. Read off the output, it is what
the screen does with the picture, rather than a share worked out again from
the window's two ends on the input. A red flower blows its red channel long
before its luminance goes, and the plane that has piled into the end bin is
the one the eye finds on the plot. Nothing on the panel writes it yet: the
plots carry no numbers, and the marks below say where the picture is
clipped rather than how much of it.

On a quantized file under a window whose ends sit on codes, the share is
the input's share at the window's to within the rounding of a share of a
pixel: a code pushed to black or past white lands wholly in an end bin of
the output, and the codes just inside are pulled further apart than a bin
and spread no further than one — a test in `stats.rs` holds the two to each
other. On a float axis the share is to the nearest fine bin.

### The marks on the picture

`w`, or the button beside the Output section's band, paints the clipped
pixels on the picture: `MARK_WHITE` and `MARK_BLACK` in
`shaders/image.wgsl`, where any channel of the windowed value is at or
past the bound, painted in place of the pixel and before the false color,
whose ramp does not end in white and black. The whole test lives in the
image shader because that is the one place the windowed value exists per
pixel; the composite pass sees the image target, which for a false-colored
file already holds the ramp's color.

A toggle, like the grid: a way of looking at the picture rather than a
setting of it. `Panels::mark_clipped` is the state, beside the plane and
count-axis toggles, `shader_codes::marks` the two bits, and
`Draw::mark_clipped` the way in — not a field of `Display`, which is kept
per file and restored when a file comes back, where the marks stay on across
files and are left alone by every reset. The key and the button both go
through `App::press(Control::Marks)`, so the two cannot drift.

The button is at the start of the row at the panel's head, and paints the Output band's two ends — the pixels the screen has
taken to black and to white — on the picture. The button wears
Lucide's `triangle-alert` (`icon::TRIANGLE_ALERT`), the sign every editor's
clipping warning wears.

Which ends are marked follows `Display::clips_white`, the same rule the
bottom bar's **clipped** uses: white is only marked where the
surface is actually clipping it.

One channel at an end is enough to mark a pixel, as it is enough for
`Plot::clipped` to count it. A channel that has reached the bound has lost what it
held, and the color left over is not the file's: a raw's white balance
lifts its red well above its green, so a sun's red burns out while its green
is still climbing and the disc comes out pink, and on a warm sunrise the
green may never reach white at all — a test that waited for every channel
painted nothing on a picture whose sun was plainly gone. The paint and
`Plot::clipped` then answer the same question, the one as a place and the
other as a share of the picture wearing the paint. The comparisons are
exact, as `Plot::clipped`'s are.

The test is made in the working space's channels — the color after
`params.primaries` has carried it into BT.709, which is the color `shade`
windows — because that is where the band's two ends are, and where
`Plot::clipped` counts: `Stats::scan` measures the picture there too. A vivid Rec.
2020 or P3 color has a BT.709 channel above white, and one below zero, that
the file never clipped but a surface that stops at either end will, and the
mark says so; [color management](color.md) says why that is a highlight's
clip and not a gamut matter of its own, and what becomes of the color on a
surface with the room. Such a texel is past both ends at once, and the two
marks are painted as a partition of the pixel, so `judge` gives it one:
white where white is being painted, and black otherwise — which is why the
paint's gate, `shader_codes::marks`, is part of `MarksKey`, and a curve
switched on rewrites the chain. The matrix keeps white, so a BT.709 file is
judged exactly as its codes are stored.

The mark is a texel's, not a blend's, and it is kept as one. `judge` in the
image shader is the verdict on one texel of the picture as uploaded, under
the window in force — through the same `load` and `expanded` the draw reads
by, so on lifted, straight color — and everything the marks show is that
verdict, averaged over exactly the texels the picture is averaged over.
Magnifying, a pixel lies inside one texel and takes its verdict outright.
Minifying at the picture's own level, up to the chain's step, `marks_over`
judges the texels under the pixel live, over `area`'s footprint. Farther
out the draw reads a coarse level of the picture, whose texels are averages
that no longer say which of their texels were at an end, so the marks have
a coarse chain of their own (`image_layer::Marks`): `fs_marks` writes its
first level from the picture, each texel the shares of the `STEP` by `STEP`
block under it, and the reducer makes the rest from that as it makes the
picture's chain from the picture, in `Rg8Unorm`, white in one channel and
black in the other. Every coarse level of the picture is bound beside the
marks at the same level, and `shade` wears the paint by the share it reads.
A run of crushed texels stays whole blue at every zoom, a scattered shadow
thins into its neighbors as the crushed pixels thin among them, and nothing
changes at 1:1.

Nothing is stored per texel of the picture — the chain begins at a
sixteenth of it, about a bit a texel — and nothing is stored at all while
the marks are off or nothing coarse is drawn, so a black point dragged at
1:1 costs the marks nothing, and dragged on a fitted view costs one pass
over the picture and the chain from it, the same as the picture's own chain
costs on every frame of an animation. `MarksKey` is what the chain was
written under, the window transform and the lift's weight, and
`ImageLayer::prepare` writes it again when that changes — into the textures
it has, through `Reducer::build_into`, since a window stepped is the same
picture judged again and allocates nothing — and drops it when the texels
change or it is not wanted; every level is rebound only when the chain's
textures change, which a rewrite does not. The
pass reads the picture through a bind group of the chain's own
(`Marks::reading`): the picture's level-1 group names the marks' first level
beside it, and a texture cannot be a pass's target and its input at once.

Two things this is instead of. Judging the resampled color would have found
an end only under a pixel every texel of which is at it: a scattered shadow
vanished the moment the view dropped below 1:1, and at exactly 1:1 came and
went with the rounding, since where a pixel's center falls in the texture is
arithmetic on its coordinate that is not exact at the far end of a large
picture, and `antialiased_nearest` there blends a thousandth of the neighbor
in — shadows that sparkled as the picture was zoomed and went missing at
100%. And reading the marks off the picture's own texels under a far-out
pixel would have cost every texel under it, which is what the chain exists
to avoid. The paint is laid over the pixel's own color, the average of every
texel under it, marked ones included, so a pixel partly marked is a hair off
what painting every texel first and shrinking would give; one wholly marked,
or not at all, is exactly that.

## The Display section

The response curve is drawn always. On a display with nothing asked of it
the curve is the diagonal, which in a panel that plotted one stage would
say nothing the axis under it did not; here the section is the display, and
its curve is what it is read for, a diagonal saying the display does
nothing as plainly as a bend says what it does.

### The rows are every file's

Three rows of settings under the Display section's band — *Exposure*,
*Window*, *Curve* — and every file gets all three, so the section is one
height for every file and for the whole of a file's stay. A section whose
controls jumped from one file to the next, or every time a handle was
dragged past white, would be one no one could keep a hand on.

Three kinds of file use this program — `AutoWindow::default_for` splits on
`Referred`, and the colormaps, the float TIFF and the GeoTIFF keys say who
the measuring audience is — and a graded file's window is 0..1 by rights
where a measurement is windowed to what it holds. Scene light — a Radiance
picture, an EXR — keeps 0..1 too, in the file's own units, and opens with
the exposure already turned: `Display::for_image_with` meters it, putting
the key of the scene at middle gray, and the slider shows the reading in
stops. The handles do not follow that split, because the black handle moves
the window on every file: a graded file's window leaves 0..1 at the bottom
the moment the black handle is touched, so a white handle that meant the
exposure on a graded file and the window's top on a measured one would be a
handle that meant a different thing on the next file along. The two dials
are honestly two. The window is in the file's own units — both handles,
both pairs of keys, the *Window* row — and the exposure is in stops — the
slider, `d`/`f`, `--exposure` — a push on top of whatever the window is,
which is exactly what a meter's reading is, and why a metered file arrives
with the exposure dial turned rather than a fourth kind of window.

The *Window* row of rules does follow the split: it is drawn only on an
image read as data, and the rules are refused on a photo, the button
(`tooltip::PHOTO_AS_STORED`) and `e` (`When::Data`) alike. A rule is an
answer to the question a measurement raises — where, in numbers with no
white, is the range worth looking at — and a photo does not raise it: its
window is 0..1 by rights, *Full range* on it is the handles put at the
darkest and brightest pixel, which a finished picture nearly always spans
already, and *Trimmed* is an auto-levels that makes it look wrong rather
than show anything a drag on the handles does not. Dragging the handles is
how a photo's shadows and highlights are looked into, and the reset is what
puts a hand-moved window back there, as it puts everything back. The
interface treats the file's three readings as two, photo and data, for the
same reason: scene light and graded light differ in how they open — the
meter — and in nothing a control on the panel does.

The curve row is every file's because the exposure is: a stop up puts the
top of any file above white and the bar says **clipped**, and the curve is
the answer to that. A row offered only to a file that opens above white
would leave `t` putting a curve on a file whose panel had no row for it, and
the bottom bar naming a curve there was no button for. It is not the
false-color case, where the key is dead because the curve does nothing; on
an 8-bit file at 0 EV the neutral curve still takes its offset out of the
shadows, which is something, and the response curve on the plot shows it.
The row is *Curve*, and its two cells are *Clip* and *Roll off*, since what
the row chooses is what becomes of the light above white on a surface that
stops there, and a cell that named the curve would name a thing to look up
rather than a thing to see. There is one curve to roll off with —
[color.md](color.md) says why only one.

The three windows are named for what they do — *As stored*, *Full range*,
*Trimmed* — rather than for the rule that does it, which the tooltip says,
and the bottom bar says the same in one word each: *stored*, *full*,
*trimmed*, and *manual* once a handle has moved. There is no fourth button
for "the image's own", because the file's own is always one of the three —
*As stored* on a graded file and on scene light, *Trimmed* on a measurement
— and the reset button puts it back, the meter's exposure with it. The one
measurement that opens on *Full range* is a gain map shown in the picture's
place (`DecodedImage::opens_on`): its every value is one its metadata
defines, so there is nothing stray for a trim to throw out, and the tail a
trim clips is where the map lifts most.

### The band is the levels track

The band under the plot is the axis — what the display makes of each value
along it — and the handles on it mark the window, which makes the band the
levels track every editor has: drag the black handle to set what comes out
black, the white handle to set what comes out white, the band between them
to slide the window along. Nobody has to be told what the two handles do,
where the same four operations spelled out as a reading and four chevrons
would have to be explained.

The handles stand at the *displayed* bounds — `Display::displayed_bounds`,
exposure folded in — because those are the values the band goes black and
white at, and a handle that stood anywhere else would mark a value the shader
is not clipping at. A handle is dragged *to* the pointer rather than *by* it —
`Command::BlackPoint` and `Command::WhitePoint` carry the value the hand asks
for, on every frame of the drag — so the interface holds no state about the
drag and a hand that runs off the end of the band puts the handle at the end.
The band's own drag, `Command::Slide`, is by the hand's movement, since what
it is sliding is wherever the window already was.

Each handle moves its own end of the window and nothing else:
`Display::put_black` and `Display::put_white`, twins. Exposure and the
window's top are two dials for one effect — a stop up is white moved to
half its value with black held — and they stay two dials, because they
answer differently to everything else: the window is found from the pixels
and found again when the file changes on disk or a *Window* rule is pressed,
and the exposure survives that as the push on top, which is what lets
`--exposure -1` mean the same thing over a directory of frames that each
keep their own trimmed window. `Display::set_displayed_bounds` works the
window back from the two displayed values with the exposure left as it is,
so a hand-set white under a stop of exposure is that white with the stop
still on top of it.

The slide stops at the plot's ends, as the handles do because the band
does. A window slid off what is plotted makes nothing black, or nothing
white: a lift, which is a grading operation and not a place to look. So the
band pans within the plot — *which part of the
range, at this width* — which is only a question once the window is
narrower than the plot: after some exposure on a graded file, or a trimmed
or hand-set window on data. A window as wide as the plot does not move,
since there is nowhere for it to go.

The keys are the handles' twins. `a`/`s` step the black handle and `A`/`S`
the white one, each by a twentieth of the window's width along the plot
(`input::WINDOW_STEP`) and no further than the plot goes, through
`Display::step_black` and `Display::step_white`. Along the plot, on the
file's own curve, and not through linear light: the plot gives the shadows
most of the band on a graded file, so a twentieth of the *light* is a
quarter of the band on the first press from 0 and a tenth on the next, the
handle bounding out of the shadows and then slowing. Stepped on the curve
it moves the same distance on the band every time, as a drag does. A step
that the plot's end stops short at is no step, and says so by returning
`false` so the key does not redraw; a press at the end asks for the end,
and the end can stand a rounding error off the handle, so a move of a hair
is no move either. The keys are in the handles' basis — an end each —
rather than window and level, so that each handle's tooltip can name a key
that does what the handle does. Nothing slides the window from the
keyboard, and the band's tooltip says so by naming no key. A narrowing is
two presses.

A window can still end past what is plotted, which a few stops of exposure
the other way is enough to do. Such a handle is drawn hollow at the edge it
went out of: it can still be taken hold of and brought back, and it does not
claim a boundary the curve running on past it says is not there.

The value under the hand is written in the section's header rather than in a
tooltip. egui takes a tooltip down for the length of a drag, and a drag is
exactly when the number is wanted.

### What goes dead

The exposure's slider is dead on a single-channel measurement, and says why
when rested on (`MEASUREMENT_NOT_EXPOSED`): a stop is a doubling of light,
and its values are not light; and over the gray ramp a stop up is exactly
the white handle brought in to the window's middle, which the handle does
in the file's own units. `Display::has_exposure` is the one test, and
`Display::set_exposure` the one refusal, so the slider, `d` and `f` and
`--exposure` cannot disagree; the bar's EV item never appears for such a
file, since the exposure stays at nothing.

The row of curves is dead under a false color, and on a single-channel
measurement, which is read through the gray ramp, and says why when rested
on — a reason each (`FALSE_COLOR_CLIPS`, `MEASUREMENT_CLIPS`), the false
color's first where both hold, being the one the hand chose. The compositor
holds the curve at a clip there — `Display::curve_on` is where the choice
is made, and `composite.rs` and the pointer's readout both ask it, both
reading `Display::ramp`; the bar's words read `Display::false_colored`,
since the gray ramp is nothing to name — because a ramp has no color past
its end for a highlight to roll off into, and a curve over the ramp would
bend the very mapping the reading is being taken off. `Display::response` runs the curve
the compositor runs, the buttons refuse the press and `t` does nothing, and
the row stays where it is — the section's height is the file's — rather
than leaving. A panel that lit whichever curve was chosen and let the buttons
change it would have a press change nothing on screen and then change the
picture some time later, when the ramp came off.

### The exposure is a slider

A spinner — a number with a step either side of it — is the right control
for a value that is mostly typed and occasionally nudged; the exposure is
neither. It is swept — up until the shadow shows something,
back until the highlight stops clipping — and a sweep wants a line with a
handle on it, where a length is the push and its direction is which way.
So the row is a slider: a groove with a mark at nothing, the run from there
to the handle filled in the accent, and the handle drawn as the band's are
because it is the same kind of thing. The reading stays, at the end of the
row, since a slider with no number is a slider you have to read the picture
to check.

It runs six stops each way (`SLIDER_STOPS`), not the sixteen the keys and
`--exposure` go to. A step is a quarter of a stop, and across the room the
row has that is a few pixels each at six, which is a slider that can be
put on a value, and under two at sixteen, which is not; and six is the
whole of what a viewer does with an exposure, a shadow lifted out of black
or a highlight brought back from four stops over. It is drawn dead on a
single-channel measurement, which has no exposure (see *What goes dead*). Past the end the handle
stands hollow at it, as a band handle out past the plot does, and the
reading says where the exposure really is. The slider is dragged *to* the
pointer, as the handles are, and asks through `Command::Exposure` only for
an exposure that is not already the one in force, so that a hand resting
on it is not a redraw a frame. It snaps to the quarter stops, so the
reading beside it is always one the keys could have reached, and so that
the keys and the slider cannot take the picture to two different places
that read the same.
## How a plot is drawn

Each plane is drawn as a line along its top (`plot::lines`), 1.5 device
pixels wide, over an area laid down under it in a faint gray taken some way
toward the line's own color (`plot::line_area`). The areas stack, so where
the planes overlap the plot darkens to a neutral gray and where one stands
alone it wears that plane's cast. Every plane is always drawn, the
luminance plane in a neutral gray and first, so that its line runs under the
color planes'.
The planes' inks are a red, a green and a blue held short of the primaries
(`plot::HISTOGRAM_PLANES`), each leading in its own channel so that three
lines crossing stay three; the full primaries, at a pixel to the bin, come
out as a hedge of pure red, green and blue spikes that the eye cannot leave
alone.
The ground is the same in every theme and so are the inks, so the reading is
the same everywhere — see [theme](theme.md) for the two things that resist
being themed. A bin is a point, which fixes the panel's width and is what
the pointer reads the plot by. What is drawn is one column to the device pixel (`icon::Grid::columns`),
each as tall as the fullest bin under it (`plot::bins_under`,
`plot::tallest`): a column to the bin lands half its edges mid-pixel at a
fractional scale, and the feathering shows as a comb, where a column to the
pixel is even at any scale; and taking the fullest bin rather than the mean
keeps a spike from being averaged away where a column is wider than a bin.
At one device pixel to the point the two are the same picture. The bands
under the plots and a ramp's swatch are cut the same way, which is why
nothing here smooths the plot.

### The count axes

A bar's height is the share of its plane's pixels in its bin, against a top
(`plot::Counts`). The input's two plots, the Display's and the Image's,
share one (`histogram::input_counts`), so a bar of one height is the same
share of the picture in each, and what the gain map's lift and the working
space do to the file read as bars growing or shrinking between them. Both
are the input, which nothing set on the panel changes, so their scale holds
still while the window, the exposure and the curve are dragged about.

The Output's top is its own, worked out again from what goes out. A scale
held to the input's is the wrong one for it: a display that stretches the
picture — a stop of exposure up, a narrowed window — spreads it over more of
the Output's axis, and its bars would fall to a tenth of the plot, on the
section that answers the panel's main question, while it is being set. The comparison it gave up, Display against Output
bar for bar, is what the curve over the Display plot already draws.

Each top is set the same way: of a plot's bins holding anything inside the
two end bins, across every plane, the fullest fiftieth (`plot::CUT`) stand
past it, and two plots sharing one take the higher (`Counts::wider`), the one
cutting fewer bars off. The ends are left out because what the window throws
away piles up there. A bar
past the top runs to it with a cap in the accent, and the pointer on it puts
its share in the header ahead of the bin's value, which is all the height of
a cut bar no longer says.

Linear, because heights only compare linearly. A logarithmic switch was the
plot's answer to one bin flattening the rest — a masked sea, a scan's
surround, a flat sky — and the cut is a better one: the shape stays the
distribution, and the spike says what it holds when asked.

The gain map's plot has a top of its own, worked out the same way: it is of
another image, the map, in stops of lift, and no height on it compares with
one on the picture's.

