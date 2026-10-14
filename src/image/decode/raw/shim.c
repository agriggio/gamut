/* The fields of LibRaw's data that have no accessor in its C interface
 * and sit too deep in `libraw_data_t` to be reached by transcribing the
 * struct: the picture as unpacked — the mosaic as the sensor made it, or a
 * Linear Raw's three values a pixel — and the black levels. Compiled
 * against the installed library's own header by `build.rs`, so the offsets
 * are the library's whatever version it is; `ffi.rs` declares these five
 * and nothing else of its own. */

#include <libraw/libraw.h>

/* `rawdata.raw_image`: one count a photosite, `raw_width` by `raw_height`
 * at `raw_pitch` bytes a row, after `libraw_unpack`; null for a sensor
 * that is not a mosaic — Foveon, a linear DNG, a float raw. */
const unsigned short *gamut_libraw_raw_image(const libraw_data_t *data)
{
    return data->rawdata.raw_image;
}

/* `rawdata.color3_image` and `rawdata.color4_image`: a picture stored
 * demosaiced — a Linear Raw DNG — once unpacked, three or four values a
 * pixel, `raw_width` by `raw_height`, through the linearization table
 * already. Which of the two the library fills is its version's choice, and
 * the other is null; both are null for a mosaic. */
const unsigned short *gamut_libraw_color3_image(const libraw_data_t *data)
{
    return (const unsigned short *)data->rawdata.color3_image;
}

const unsigned short *gamut_libraw_color4_image(const libraw_data_t *data)
{
    return (const unsigned short *)data->rawdata.color4_image;
}

/* `color.black`: the black level every photosite shares. */
unsigned gamut_libraw_black(const libraw_data_t *data)
{
    return data->color.black;
}

/* `color.cblack`: what each color's black adds to it — the first four —
 * and after them a pattern of blacks over the frame, its two dimensions
 * at 4 and 5 and its values from 6. `LIBRAW_CBLACK_SIZE` long. */
const unsigned *gamut_libraw_cblack(const libraw_data_t *data)
{
    return data->color.cblack;
}
