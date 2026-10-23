// Reading one texel of the image as uploaded, shared by the image layer and
// the coarse chain: `load` finds it, lifts it where the picture has a gain
// map, and premultiplies it. Prepended to `image.wgsl` and `reduce.wgsl` by
// `render::mod`, so that the two cannot drift; each declares the `params`,
// `source`, `gain_map` and `gain_table` this reads, at the names used here.
// `covered` and its two neighbors are the no-data sentinel's: what an area
// average leaves out, as the thumbnails' `image::resample` leaves it out.

// Color premultiplied by alpha, in the component layout the texture stores.
// Every filter that reads it is a weighted sum of texels, and weighting
// straight alpha would drag the color of fully transparent texels into their
// neighbors, which is what shows as haloing along a hard edge. Coarse levels
// are already premultiplied, so this is a no-op for them.
fn premultiplied(texel: vec4<f32>) -> vec4<f32> {
    if params.alpha_mode != 1u {
        return texel;
    }
    switch params.swizzle {
        case 1u: { return vec4<f32>(texel.r * texel.g, texel.g, texel.ba); }
        case 3u: { return vec4<f32>(texel.rgb * texel.a, texel.a); }
        default: { return texel; }
    }
}

// The gains the table gives one value of the map, per channel: for a
// one-channel map the three gains of its one value; for a three-channel
// map each channel's gain of its own value; and for a map of ratios, a
// raw's profile, the gains themselves, read without the table.
fn gains(coord: vec2<i32>) -> vec3<f32> {
    let value = textureLoad(gain_map, coord, 0);
    if params.lift == 4u {
        return value.rgb;
    }
    let at = vec3<i32>(round(value.rgb * 255.0));
    if params.lift == 1u {
        return textureLoad(gain_table, vec2<i32>(at.r, 0), 0).rgb;
    }
    return vec3<f32>(
        textureLoad(gain_table, vec2<i32>(at.r, 0), 0).r,
        textureLoad(gain_table, vec2<i32>(at.g, 0), 0).g,
        textureLoad(gain_table, vec2<i32>(at.b, 0), 0).b,
    );
}

// The gain at texel `coord` of the image as uploaded: the map's four
// surrounding values, each through the table, blended by the texel's
// distance from them. The twin of `GainMap::gain_at` in image/gain_map.rs;
// keep the two in step.
fn gain(coord: vec2<i32>) -> vec3<f32> {
    let position = vec2<f32>(coord) / params.extent * params.map_size;
    let last = vec2<i32>(textureDimensions(gain_map)) - vec2<i32>(1);
    let near = min(vec2<i32>(floor(position)), last);
    let far = min(near + vec2<i32>(1), last);
    let fraction = position - floor(position);
    let top = mix(gains(near), gains(vec2<i32>(far.x, near.y)), fraction.x);
    let bottom = mix(gains(vec2<i32>(near.x, far.y)), gains(far), fraction.x);
    return mix(top, bottom, fraction.y);
}

// One texel, lifted where the picture has a gain map — before it is
// premultiplied, filtered or converted, since the map multiplies light —
// and premultiplied.
fn load(coord: vec2<i32>) -> vec4<f32> {
    let limit = vec2<i32>(textureDimensions(source)) - vec2<i32>(1);
    let clamped = clamp(coord, vec2<i32>(0), limit);
    var texel = textureLoad(source, clamped, 0);
    if params.lift != 0u {
        let lifted = (texel.rgb + params.base_offset.rgb) * gain(clamped) - params.alternate_offset.rgb;
        texel = vec4<f32>(lifted, texel.a);
    }
    return premultiplied(texel);
}


// Whether the texel at `coord` of the picture as uploaded is data rather than
// the file's no-data sentinel, as 1 or 0: held to `nodata`, the sentinel as
// a texel reads and how far one may be from it (see `upload::Sentinel`),
// before any lift, since the sentinel is a value the file stores. A color
// texel is the sentinel only in all three channels, as
// `DecodedImage::is_nodata` has it, so that a pure red in a picture whose
// sentinel is zero is still red.
fn covered(coord: vec2<i32>) -> f32 {
    let limit = vec2<i32>(textureDimensions(source)) - vec2<i32>(1);
    let texel = textureLoad(source, clamp(coord, vec2<i32>(0), limit), 0);
    let near = abs(texel.rgb - vec3<f32>(params.nodata.x)) <= vec3<f32>(params.nodata.y);
    if params.swizzle == 0u {
        return select(1.0, 0.0, near.r);
    }
    return select(1.0, 0.0, all(near));
}

// `texel`, as `load` gave it, in the form a coarse level of a picture with a
// sentinel holds: the color times the share `covered` of it that is data,
// and that share where alpha would be — the second component for gray, the
// fourth for color. Only a picture with no alpha has a sentinel, so the
// slot is free. Averaging texels in this form is exact however many levels
// down: the mean of the data, times the share that is data.
fn with_coverage(texel: vec4<f32>, covered: f32) -> vec4<f32> {
    var out = texel * covered;
    if params.swizzle == 0u {
        out.g = covered;
    } else {
        out.a = covered;
    }
    return out;
}

// A texel in that form, resolved back into the picture's own: the mean of
// the data under it, opaque, or — where none of it is data — the sentinel,
// which the window then draws as it draws the sentinel at full size. So a
// thin line among no data keeps its own value however far out the view is,
// as it does in the thumbnails, rather than being averaged into the
// sentinel until the window loses it.
fn resolved(texel: vec4<f32>) -> vec4<f32> {
    let share = select(texel.a, texel.g, params.swizzle == 0u);
    if share <= 0.0 {
        return vec4<f32>(vec3<f32>(params.nodata.x), 1.0);
    }
    return vec4<f32>(texel.rgb / share, 1.0);
}
