/* The two fields of LibRaw's data that have no accessor in its C
 * interface and sit too deep in `libraw_data_t` to be reached by
 * transcribing the struct: the mosaic as the sensor made it, and the black
 * levels. Compiled against the installed library's own header by
 * `build.rs`, so the offsets are the library's whatever version it is;
 * `ffi.rs` declares these three and nothing else of its own. */

#include <libraw/libraw.h>

/* `rawdata.raw_image`: one count a photosite, `raw_width` by `raw_height`
 * at `raw_pitch` bytes a row, after `libraw_unpack`; null for a sensor
 * that is not a mosaic — Foveon, a linear DNG, a float raw. */
const unsigned short *gamut_libraw_raw_image(const libraw_data_t *data)
{
    return data->rawdata.raw_image;
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
