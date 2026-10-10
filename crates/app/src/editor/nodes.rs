// SPDX-License-Identifier: GPL-3.0-or-later

//! One object, one set of render nodes (`spec/05` §6).
//!
//! Every function here appends to a `gtk::Snapshot` and returns nothing. That shape is
//! the point: the *canvas* and the *export* both call [`append_scene`], so `spec/05` §6's
//! "preview == export by construction" is not a discipline anyone has to remember. The
//! only difference between the two paths is which snapshot they hand in and at what
//! scale, and `spec/05` §11 item 2 -- "pixel-compare export with a screenshot of the
//! canvas at 100 % zoom" -- is a test of exactly that.
//!
//! Nothing here decides *order*. `octosnap_scene::Scene::render_order` does, in the
//! crate that can be tested without a display; this file walks the list it is given. A
//! renderer that sorted for itself would agree with the scene until one of them was
//! edited.

use gtk::prelude::*;
use gtk::{cairo, gdk, graphene, gsk};
use octosnap_scene::redact as ops;
use octosnap_scene::style::{ArrowHead, RedactStyle, Rgba, SpotlightShape, arrowhead_length};
use octosnap_scene::{
    Background, BackgroundParams, Bounds, Geometry, Object, Placement, Point, Scene, magnifier,
};

use super::redact::Raster;

/// Which of the document [`append_scene`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    All,
    /// The background, the base image and the picture's own objects -- redactions,
    /// inserted pictures, spotlights -- which a vector export flattens into one image, so
    /// nothing a redaction covers is left in the file under it.
    Picture,
    /// The annotations over the picture, which a vector export keeps as vectors.
    Marks,
}

/// Where a magnifier's lens takes the picture it enlarges from (`ANN-32`, D187).
#[derive(Debug, Clone, Copy)]
pub enum Lens<'a> {
    /// The picture as [`append_scene`] draws it, drawn once more for the lenses: the
    /// canvas's, and an export to pixels.
    Drawn,
    /// The picture already flattened with its redactions -- a vector export's -- of the
    /// canvas `canvas` at `scale` pixels to the unit. Each lens takes the pixels under its
    /// spot from it, so the file holds those few and not the picture again, nor any of what
    /// a redaction covers.
    Flattened { picture: &'a gdk::Texture, canvas: Bounds, scale: f64 },
}

/// Draws the whole document, bottom to top (`spec/05` §5.3).
///
/// `base` is the capture's texture, uploaded once by the caller and reused every frame --
/// `spec/05` §6 is explicit that it is "one `GdkTexture` uploaded once". Passing it in
/// rather than loading it here keeps this file free of I/O and makes the export path use
/// the very same texture the canvas is showing.
///
/// **This function owns the order and nothing else.** `draw` is called once per object and
/// is responsible for putting it on the snapshot, which is what lets the caller cache a
/// node per object (§6: "each object builds a cached `RenderNode` when it changes;
/// unchanged objects reuse their node") without this file knowing that a cache exists.
///
/// The callback used to be `glyphs`, called *after* `append_object` to add the text and
/// counter glyphs -- a Pango layout needs a font map and this file has none. That split
/// cannot survive a per-object cache, because the cached node has to contain the whole
/// object: a rectangle's stroke and a label's glyphs are one node or they are two things
/// that can be reordered relative to each other, which is the bug the callback was
/// introduced to fix in the first place. So `draw` now covers the object entirely, and
/// `spec/05` §5.3 still decides when it is asked for.
///
/// `part` is all of it, or one of the two layers a vector export keeps apart (`ANN-33`,
/// D184). The split is `ObjectKind::framed`'s, the one the picture's frame already makes
/// (D166), so the two parts drawn one over the other are the whole.
pub fn append_scene(
    snapshot: &gtk::Snapshot,
    scene: &Scene,
    base: Option<&gdk::Texture>,
    draw: &dyn Fn(&gtk::Snapshot, &Object),
    part: Part,
    lens: Lens<'_>,
) {
    // Once, and threaded through everything below it. Asking four times per frame -- twice
    // here and twice inside the spotlight mask -- was an allocation and a sort each time,
    // plus a linear `Scene::get` per id to turn the ids back into objects (D54).
    let order = scene.render_list();
    if part != Part::Marks {
        append_picture(snapshot, scene, base, draw, &order);
    }
    let annotations: Vec<&Object> = order
        .iter()
        .copied()
        .filter(|o| !matches!(o.geometry, Geometry::Background { .. }) && !o.kind().framed())
        .collect();
    if part == Part::Picture || annotations.is_empty() {
        return;
    }
    // Then the annotations, in the order the scene decided, over the whole canvas.
    let place = scene.placement();
    // `ANN-32`'s lenses show the picture again, enlarged: drawn once for all of them, in
    // the picture's coordinates, which are the marks'.
    let lenses = annotations.iter().any(|o| matches!(o.geometry, Geometry::Magnifier { .. }));
    let drawn = (lenses && matches!(lens, Lens::Drawn))
        .then(|| {
            let inner = gtk::Snapshot::new();
            on_the_canvas(&inner, place, |inner| append_picture(inner, scene, base, draw, &order));
            inner.to_node()
        })
        .flatten();
    on_the_picture(snapshot, place, |snapshot| {
        for object in &annotations {
            if !matches!(object.geometry, Geometry::Magnifier { .. }) {
                draw(snapshot, object);
                continue;
            }
            let content = match lens {
                Lens::Drawn => drawn.clone(),
                Lens::Flattened { picture, canvas, scale } => {
                    flattened_spot(object, picture, canvas, scale, place)
                }
            };
            append_magnifier(snapshot, object, content.as_ref());
        }
    });
}

/// The picture: a background, then the base image and the picture's own objects inside
/// §4.13's frame. [`append_scene`]'s first part, and what a lens enlarges (`ANN-32`).
fn append_picture(
    snapshot: &gtk::Snapshot,
    scene: &Scene,
    base: Option<&gdk::Texture>,
    draw: &dyn Fn(&gtk::Snapshot, &Object),
    order: &[&Object],
) {
    // The base image, under everything except a background object. `spec/05` §5.3 puts
    // the background at group 0 and the base at group 1, and the scene's order already
    // reflects that -- so a background is appended before this and shows around a canvas
    // that a crop has made larger than the capture (§11 item 7).
    //
    // The background is drawn in the *canvas's* coordinates, and everything above it in
    // the *picture's*. The two are the same space until `spec/05` §4.13's inset shrinks
    // the picture inside the canvas, and then they are not -- so the transform goes here,
    // around the base image and every object at once, rather than into each of them.
    // Around all of them, because an arrow pointing at a button has to keep pointing at
    // it: the annotations are on the picture, not beside it.
    for object in order {
        if matches!(object.geometry, Geometry::Background { .. }) {
            draw(snapshot, object);
        }
    }

    // `spec/05` §4.13's rounded corners and shadow are properties of the *picture*, so
    // they wrap it: the shadow is cast by the clip's silhouette, which is why the clip
    // goes inside the shadow and not beside it.
    //
    // The frame holds the base image and the picture's own objects, and no annotation
    // (D166). An arrow drawn on the background's margin is outside the picture's corners,
    // and inside the clip it vanished as it was drawn, behind a background that is always
    // at the bottom. Outside the shadow too, so an arrow does not cast the picture's
    // shadow onto the gradient; its own shadow is its style's (§5.1). The split changes
    // no order: `ObjectKind::framed` holds for a prefix of the render list.
    //
    // At square corners too (D189). The clip was pushed only for a radius, and without it
    // a background over a crop showed the capture beyond the crop on its margin, and a
    // spotlight's scrim there. Not for the crop's own fill, which frames nothing (D191).
    let mut pushed = 0_u8;
    if let Some((rect, radius, shadow)) = picture_frame(scene) {
        if let Some(shadow) = shadow {
            snapshot.push_shadow(&[shadow]);
            pushed += 1;
        }
        snapshot.push_rounded_clip(&rounded(rect, radius));
        pushed += 1;
    }
    on_the_picture(snapshot, scene.placement(), |snapshot| {
        // The crop's own fill, which a background the panel laid out in its slot keeps: the
        // area a crop past the image added is the picture's, filled as the crop left it, and
        // the frame's corners and shadow are the whole rect's (D192).
        if let Some((rect, color)) = scene.frame_fill() {
            snapshot.append_color(&to_rgba(color), &to_rect(rect.normalised()));
        }
        if let Some(texture) = base {
            append_base(snapshot, texture, &scene.base);
        }
        for object in order {
            if !matches!(object.geometry, Geometry::Background { .. }) && object.kind().framed() {
                draw(snapshot, object);
            }
        }
    });
    for _ in 0..pushed {
        snapshot.pop();
    }
}

/// `body`, drawn in the canvas's coordinates, inside the picture's: the inverse of
/// [`on_the_picture`], for the picture a lens enlarges among the marks (`ANN-32`), and what
/// a redaction's raster is cut from (D189).
pub(super) fn on_the_canvas(
    snapshot: &gtk::Snapshot,
    place: Placement,
    body: impl FnOnce(&gtk::Snapshot),
) {
    if place.is_identity() || place.scale <= 0.0 {
        body(snapshot);
        return;
    }
    snapshot.save();
    #[allow(clippy::cast_possible_truncation)]
    snapshot.scale((1.0 / place.scale) as f32, (1.0 / place.scale) as f32);
    #[allow(clippy::cast_possible_truncation)]
    snapshot.translate(&graphene::Point::new(-place.x as f32, -place.y as f32));
    body(snapshot);
    snapshot.restore();
}

/// `ANN-32`'s magnifier: the spot's ring and the line to the lens, then the lens -- white
/// where its spot is off the picture, the picture enlarged about the spot, and the ring
/// over its edge (`octosnap_scene::magnifier`). `content` is the picture, in the picture's
/// coordinates; without it the lens is the ring around white.
#[allow(clippy::cast_possible_truncation)]
fn append_magnifier(snapshot: &gtk::Snapshot, object: &Object, content: Option<&gsk::RenderNode>) {
    let Geometry::Magnifier { center, radius, source, zoom } = object.geometry else { return };
    let color = to_rgba(object.style.color);
    let width = object.style.stroke_width();
    // The spot and the line are a hairline beside the lens's ring: they point, it frames.
    let thin = (width / 2.0).max(1.0);
    if object.style.shadow {
        snapshot.push_shadow(&[shadow_for(width)]);
    }
    stroke(snapshot, &ellipse_path(magnifier::spot(source, radius, zoom)), thin, &color);
    if let Some(line) = magnifier::link(center, radius, source, zoom) {
        stroke(snapshot, &segment_path(line.start, line.end), thin, &color);
    }
    let lens = magnifier::lens(center, radius);
    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(to_rect(lens), radius as f32));
    snapshot.append_color(&gdk::RGBA::WHITE, &to_rect(lens));
    if let Some(content) = content {
        // `magnifier::through`: the spot's middle on the lens's, `zoom` times larger.
        snapshot.save();
        snapshot.translate(&graphene::Point::new(center.x as f32, center.y as f32));
        snapshot.scale(zoom as f32, zoom as f32);
        snapshot.translate(&graphene::Point::new(-source.x as f32, -source.y as f32));
        snapshot.append_node(content);
        snapshot.restore();
    }
    snapshot.pop();
    stroke(snapshot, &ellipse_path(lens), width, &color);
    if object.style.shadow {
        snapshot.pop();
    }
}

/// A vector export's lens: the flattened picture's pixels under the spot, a pixel more on
/// each side for the edge's filtering, placed in the picture's coordinates. Only these go
/// into the file with the lens.
///
/// Drawn with cairo rather than as a texture node. GSK pads a texture node's edges, which
/// cairo's SVG surface cannot say, and it put 300 dpi pictures of every band the lens
/// crossed in the file instead. A cairo node is replayed as it was drawn: the crop, one
/// picture at its own size.
pub(super) fn flattened_spot(
    object: &Object,
    picture: &gdk::Texture,
    canvas: Bounds,
    scale: f64,
    place: Placement,
) -> Option<gsk::RenderNode> {
    let Geometry::Magnifier { radius, source, zoom, .. } = object.geometry else { return None };
    let spot = magnifier::spot(source, radius, zoom);
    let corner = place.apply(Point::new(spot.x, spot.y));
    let far = place.apply(Point::new(spot.x + spot.width, spot.y + spot.height));
    let (width, height) = (f64::from(picture.width()), f64::from(picture.height()));
    let x0 = (((corner.x - canvas.x) * scale).floor() - 1.0).clamp(0.0, width);
    let y0 = (((corner.y - canvas.y) * scale).floor() - 1.0).clamp(0.0, height);
    let x1 = (((far.x - canvas.x) * scale).ceil() + 1.0).clamp(0.0, width);
    let y1 = (((far.y - canvas.y) * scale).ceil() + 1.0).clamp(0.0, height);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let crop =
        crop_image(picture, x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize)?;
    // The crop's pixels back on the canvas, and from there in the picture's coordinates.
    let from = place.invert(Point::new(canvas.x + x0 / scale, canvas.y + y0 / scale));
    let to = place.invert(Point::new(canvas.x + x1 / scale, canvas.y + y1 / scale));
    let rect = Bounds::new(from.x, from.y, to.x - from.x, to.y - from.y);
    let node = gsk::CairoNode::new(&to_rect(rect));
    {
        let cr = node.draw_context();
        cr.translate(rect.x, rect.y);
        cr.scale(rect.width / (x1 - x0), rect.height / (y1 - y0));
        cr.set_source_surface(&crop, 0.0, 0.0).ok()?;
        cr.paint().ok()?;
    }
    Some(node.upcast())
}

/// `width` by `height` pixels of `texture`, from `x`, `y`, as cairo's image: premultiplied,
/// in the byte order cairo's ARGB32 is.
fn crop_image(
    texture: &gdk::Texture,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Option<cairo::ImageSurface> {
    let format = cairo::Format::ARgb32;
    let stride = format.stride_for_width(u32::try_from(width).ok()?).ok()?;
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(if cfg!(target_endian = "little") {
        gdk::MemoryFormat::B8g8r8a8Premultiplied
    } else {
        gdk::MemoryFormat::A8r8g8b8Premultiplied
    });
    let (bytes, from_stride) = downloader.download_bytes();
    let (row, used) = (usize::try_from(stride).ok()?, width * 4);
    let mut data = vec![0_u8; row * height];
    for line in 0..height {
        let start = (y + line) * from_stride + x * 4;
        data.get_mut(line * row..line * row + used)?
            .copy_from_slice(bytes.get(start..start + used)?);
    }
    cairo::ImageSurface::create_for_data(
        data,
        format,
        i32::try_from(width).ok()?,
        i32::try_from(height).ok()?,
        stride,
    )
    .ok()
}

/// `body`, in the picture's coordinates: every object is in the picture's space, which
/// §4.13's padding moves and its inset shrinks inside the canvas.
fn on_the_picture(
    snapshot: &gtk::Snapshot,
    place: octosnap_scene::Placement,
    body: impl FnOnce(&gtk::Snapshot),
) {
    if place.is_identity() {
        body(snapshot);
        return;
    }
    snapshot.save();
    #[allow(clippy::cast_possible_truncation)]
    snapshot.translate(&graphene::Point::new(place.x as f32, place.y as f32));
    #[allow(clippy::cast_possible_truncation)]
    snapshot.scale(place.scale as f32, place.scale as f32);
    body(snapshot);
    snapshot.restore();
}

/// The capture, laid into the document the way `spec/05` §4.12's Rotate and Flip left it.
///
/// The pixels are never turned (D91): the file the editor opened is the file it still has,
/// and the turns are a transform around the one `append_texture` that draws it. Which is
/// also why a rotate costs nothing and undoes exactly -- there is no resample to undo.
///
/// The order is the orientation's own: the mirror is innermost, in the texture's space,
/// and the quarter turns are outside it. `Orientation::mirror` is the algebra that keeps
/// composing them in that order honest.
#[allow(clippy::cast_possible_truncation)]
fn append_base(snapshot: &gtk::Snapshot, texture: &gdk::Texture, base: &octosnap_scene::Base) {
    let orientation = base.orientation;
    // The texture's own rect: the document's, with the axes put back.
    let (tw, th) = if orientation.swaps_axes() {
        (base.height, base.width)
    } else {
        (base.width, base.height)
    };
    if orientation.is_upright() {
        snapshot.append_texture(texture, &to_rect(Bounds::new(0.0, 0.0, tw, th)));
        return;
    }
    snapshot.save();
    // Each turn moves the texture's rect off the origin by one of the document's sides;
    // the translate brings it back before the rotation happens.
    match orientation.quarter {
        1 => {
            snapshot.translate(&graphene::Point::new(base.width as f32, 0.0));
            snapshot.rotate(90.0);
        }
        2 => {
            snapshot.translate(&graphene::Point::new(base.width as f32, base.height as f32));
            snapshot.rotate(180.0);
        }
        3 => {
            snapshot.translate(&graphene::Point::new(0.0, base.height as f32));
            snapshot.rotate(270.0);
        }
        _ => {}
    }
    if orientation.mirrored {
        snapshot.translate(&graphene::Point::new(tw as f32, 0.0));
        snapshot.scale(-1.0, 1.0);
    }
    snapshot.append_texture(texture, &to_rect(Bounds::new(0.0, 0.0, tw, th)));
    snapshot.restore();
}

/// Where the picture sits, how round its corners are, and what it casts -- or `None` when
/// no background frames it and the picture is simply the canvas: with none, or with the
/// crop's own fill, whose frame clipped away what was put on the area the crop added (D191).
fn picture_frame(scene: &Scene) -> Option<(Bounds, f64, Option<gsk::Shadow>)> {
    let (source, params) = scene.frame()?;
    let out = octosnap_scene::layout(source, params);
    Some((out.image, out.radius, picture_shadow(out.image, params.shadow_intensity)))
}

/// `spec/05` §4.13's `shadowIntensity`, 0-100 with 28 as the default.
///
/// One number driving three, because that is what one slider can honestly control: the
/// blur grows with the intensity, the drop grows with it at a third of the rate so the
/// picture keeps sitting *on* the background rather than floating off it, and the alpha
/// tops out well short of opaque. Scaled by the picture's own size, so a 400-pixel
/// thumbnail and a 5K capture get a shadow of the same apparent weight.
#[allow(clippy::cast_possible_truncation)]
fn picture_shadow(image: Bounds, intensity: f64) -> Option<gsk::Shadow> {
    if intensity <= 0.0 {
        return None;
    }
    let strength = (intensity / 100.0).clamp(0.0, 1.0);
    // A fortieth of the shorter side at full strength: about 27 px on a 1080-tall
    // capture, which is the weight `editor-background-panel.png` shows at its default.
    let reach = image.width.min(image.height) / 40.0 * strength;
    Some(gsk::Shadow::new(
        gdk::RGBA::new(0.0, 0.0, 0.0, (0.55 * strength) as f32),
        0.0,
        (reach / 3.0) as f32,
        reach as f32,
    ))
}

/// `spec/05` §4.13's five background sources, painted over the whole canvas.
///
/// The canvas's rect and not the object's bounds, because §4.13 ends "the result is
/// rendered as a background object below the base image" -- a background *is* the canvas,
/// which is why `Object::bounds` answers an empty rect for one and why the rect comes from
/// the scene. A crop therefore reshapes the fill for free, and so does the padding slider,
/// which resizes the canvas itself.
fn append_background(
    snapshot: &gtk::Snapshot,
    params: &BackgroundParams,
    scene: &Scene,
    base: Option<&gdk::Texture>,
    wallpaper: Option<&gdk::Texture>,
) {
    paint_background(snapshot, &params.background, scene.canvas, base, wallpaper);
}

/// [`append_background`]'s paint, over any rectangle.
///
/// Public and rectangle-shaped because §4.13's panel is a wall of swatches, and a swatch
/// that previewed a gradient with its own drawing code would be a second renderer to keep
/// in step with this one. The panel draws its twenty at 56 x 40 with the same call the
/// canvas draws one at 5120 x 2880 -- which is exactly `spec/05` §6's argument for one
/// node tree, applied one level up.
pub fn paint_background(
    snapshot: &gtk::Snapshot,
    background: &Background,
    canvas: Bounds,
    base: Option<&gdk::Texture>,
    wallpaper: Option<&gdk::Texture>,
) {
    let rect = to_rect(canvas);
    match background {
        // Transparent, and that is a real answer: §4.13's wide **None** button is the
        // selected one when the document has no background, and a padding set with None
        // is a margin of nothing -- which the canvas draws as its checkerboard, the way
        // transparency looks everywhere else in this editor.
        Background::None => {}
        Background::Color { color } => snapshot.append_color(&to_rgba(*color), &rect),
        Background::Gradient { id } => append_gradient(snapshot, *id, canvas),
        // The user's own picture, loaded by the canvas the way an image object's is and
        // handed here as a raster. Covered rather than stretched -- an 800 x 600
        // wallpaper behind a 16:9 capture must not become anamorphic -- so the short axis
        // fills and the long one overflows the clip.
        Background::Image { .. } => {
            if let Some(texture) = wallpaper {
                snapshot.push_clip(&rect);
                let placed = cover(canvas, f64::from(texture.width()), f64::from(texture.height()));
                snapshot.append_scaled_texture(
                    texture,
                    gsk::ScalingFilter::Trilinear,
                    &to_rect(placed),
                );
                snapshot.pop();
            }
        }
        // §4.13's three "derive a background from the screenshot itself" swatches. The
        // capture's own texture, covering the canvas and blurred -- which is why it is
        // drawn here rather than prepared on a worker: the GPU does it per frame for
        // nothing, and a blur of the base image is the one background whose source is
        // already resident.
        Background::Blurred { strength } => {
            if let Some(texture) = base {
                // `Blur::radius` is a *fraction* of the shorter side, so that the same
                // swatch looks the same on a 400 px crop and on a 5K capture -- and
                // `push_blur` wants pixels. Handing it the fraction was a blur of a
                // sixteenth of a pixel, which is to say none at all.
                let radius = strength.radius() * canvas.width.min(canvas.height);
                snapshot.push_clip(&rect);
                snapshot.push_blur(radius);
                let placed = cover(canvas, f64::from(texture.width()), f64::from(texture.height()));
                // Grown past the canvas before blurring: a blur samples outside its
                // source and would otherwise fade to transparent at all four edges,
                // leaving a pale frame exactly where the background should be strongest.
                let bled = placed.inflated(radius);
                snapshot.append_scaled_texture(
                    texture,
                    gsk::ScalingFilter::Trilinear,
                    &to_rect(bled),
                );
                snapshot.pop();
                snapshot.pop();
            }
        }
    }
}

/// One of the twenty (`docs/decisions.md` D88): an angled sweep, then its blobs.
///
/// The sweep is a linear gradient across the canvas's diagonal at the gradient's own
/// angle, and the blobs are radial gradients that fade to transparent -- which is what
/// makes a mesh background rather than a stripe. Both are `gsk` nodes, so the whole thing
/// is resolution-independent and costs nothing to store.
#[allow(clippy::cast_possible_truncation)]
fn append_gradient(snapshot: &gtk::Snapshot, id: u8, canvas: Bounds) {
    let Some(gradient) = octosnap_scene::background::gradient(id) else { return };
    let rect = to_rect(canvas);
    let stops: Vec<gsk::ColorStop> = gradient
        .colors()
        .into_iter()
        .map(|(at, color)| gsk::ColorStop::new(at as f32, to_rgba(color)))
        .collect();
    // The axis is a unit vector; the gradient runs from one corner of the canvas to the
    // other along it, so the same gradient fills a portrait and a landscape canvas edge
    // to edge rather than banding on one of them.
    let (dx, dy) = gradient.axis();
    let centre = canvas.center();
    let reach = (canvas.width.abs() * dx).abs() / 2.0 + (canvas.height.abs() * dy).abs() / 2.0;
    let start =
        graphene::Point::new((centre.x - dx * reach) as f32, (centre.y - dy * reach) as f32);
    let end = graphene::Point::new((centre.x + dx * reach) as f32, (centre.y + dy * reach) as f32);
    snapshot.append_linear_gradient(&rect, &start, &end, &stops);

    for blob in gradient.blobs {
        let colour = to_rgba(blob.rgba());
        let clear = gdk::RGBA::new(colour.red(), colour.green(), colour.blue(), 0.0);
        let radius = (canvas.width.max(canvas.height) * blob.radius) as f32;
        if radius <= 0.0 {
            continue;
        }
        let at = graphene::Point::new(
            (canvas.x + canvas.width * blob.x) as f32,
            (canvas.y + canvas.height * blob.y) as f32,
        );
        snapshot.append_radial_gradient(
            &rect,
            &at,
            radius,
            radius,
            0.0,
            1.0,
            &[gsk::ColorStop::new(0.0, colour), gsk::ColorStop::new(1.0, clear)],
        );
    }
}

/// A source of `width` x `height` scaled to cover `into` without distorting it, centred.
fn cover(into: Bounds, width: f64, height: f64) -> Bounds {
    if width <= 0.0 || height <= 0.0 {
        return into;
    }
    let scale = (into.width / width).max(into.height / height);
    let (w, h) = (width * scale, height * scale);
    Bounds::new(into.x + (into.width - w) / 2.0, into.y + (into.height - h) / 2.0, w, h)
}

#[allow(clippy::cast_possible_truncation)]
fn rounded(bounds: Bounds, radius: f64) -> gsk::RoundedRect {
    let r = bounds.normalised();
    let radius = radius.min(r.width / 2.0).min(r.height / 2.0).max(0.0) as f32;
    gsk::RoundedRect::from_rect(to_rect(r), radius)
}

/// One object's nodes.
///
/// `base` and `raster` are for a redaction, which is the one kind that draws pixels
/// rather than geometry: its exact raster when the canvas has one (`redact.rs`), or a
/// GPU preview blurred out of the base image while the worker is still on it.
///
/// `picture` is the file this object names, decoded by the canvas: §4.13's wallpaper for a
/// background, §4.14's inserted image for an image object. One parameter for both, because
/// from here they are the same thing -- a texture the object's own `file` chose.
pub fn append_object(
    snapshot: &gtk::Snapshot,
    object: &Object,
    scene: &Scene,
    base: Option<&gdk::Texture>,
    raster: Option<&Raster>,
    picture: Option<&gdk::Texture>,
) {
    let color = to_rgba(object.style.color);
    let width = object.style.stroke_width();

    // `spec/05` §5.1's `shadow` flag, pushed around whatever the object draws so a shape
    // and its text label cast the same shadow rather than each inventing one.
    let shadowed = object.style.shadow
        && !matches!(
            object.geometry,
            // A redaction with a shadow would draw a soft edge around the thing it is hiding,
            // which points at it. A spotlight's whole job is the mask; a shadow inside it is
            // a second edge nobody asked for.
            Geometry::Redact { .. }
                | Geometry::Spotlight { .. }
                | Geometry::Background { .. }
                // A lens casts its own, around the picture it holds (`append_magnifier`).
                | Geometry::Magnifier { .. }
        );
    if shadowed {
        snapshot.push_shadow(&[shadow_for(width)]);
    }

    match &object.geometry {
        Geometry::Line { start, end } => {
            stroke(snapshot, &segment_path(*start, *end), width, &color);
        }
        Geometry::Arrow { start, end, ctrl, style, head } => {
            append_arrow(snapshot, *start, *end, *ctrl, *style, *head, width, &color);
        }
        Geometry::Rect { bounds, filled, radius } => {
            append_rect(snapshot, *bounds, *filled, *radius, width, &color);
        }
        Geometry::Ellipse { bounds } => {
            stroke(snapshot, &ellipse_path(*bounds), width, &color);
        }
        Geometry::Path { points, smoothing, highlighter, band, pressure } => {
            let pen = pressure.as_deref().filter(|_| !highlighter);
            if !append_pressed(snapshot, points, *smoothing, pen, width, &color) {
                append_freehand(snapshot, points, *smoothing, *highlighter, *band, width, &color);
            }
        }
        Geometry::Counter { center, number, style, radius, tail } => {
            append_counter(snapshot, *center, *number, *style, *radius, *tail, &color);
        }
        Geometry::Spotlight { .. } => append_spotlight(snapshot, scene),
        Geometry::Redact { bounds, style, intensity, .. } => {
            append_redaction(snapshot, *bounds, *style, *intensity, scene, base, raster);
        }
        // Text needs a Pango layout, which belongs with the widget that has a font map;
        // the canvas appends it and this arm is deliberately empty rather than drawing a
        // placeholder that would then differ between preview and export.
        Geometry::Text { .. } => {}
        // `spec/05` §4.14's inserted image. The texture is loaded by the canvas from the
        // project's assets and handed over as `picture`, for the same reason the base
        // texture is passed in rather than read here -- and through the same per-path
        // cache, because a background's image and an object's image are the same question.
        //
        // Scaled to the bounds rather than covered: §4.14 gives the object "move/resize
        // handles", so the rect the user dragged *is* the answer. The insert picks a rect
        // of the picture's own proportions, which is where the aspect is decided.
        Geometry::Image { bounds, .. } => {
            if let Some(texture) = picture {
                snapshot.append_scaled_texture(
                    texture,
                    gsk::ScalingFilter::Trilinear,
                    &to_rect(bounds.normalised()),
                );
            }
        }
        // `spec/05` §4.13's background object, which after `spec/05` §4.11's crop has one
        // job already: it is what fills the area a crop dragged beyond the image
        // (§11 item 7, D52).
        //
        // The canvas's rect and not the object's bounds, because §4.13 ends "the result is
        // rendered as a background object below the base image" -- a background *is* the
        // canvas, which is why `Object::bounds` answers an empty rect for one and why the
        // rect comes from the scene. A later crop therefore reshapes the fill for free.
        //
        // §4.13's own sources -- a gradient, a colour, one of the user's images, or the
        // capture blurred -- are `params.background`. `style.color` is kept in step with
        // a `Color` one so a project written before M6 still opens as the flat fill it
        // was, and so the swatch in the toolbar has something to show without decoding a
        // gradient.
        Geometry::Background { params, .. } => {
            append_background(snapshot, params, scene, base, picture);
        }
        // A crop does not draw: it *is* the canvas rect, and the canvas is what every
        // path here is already measured against.
        Geometry::Crop { .. } => {}
        // `append_scene` draws a lens, with the picture it enlarges; alone, it is its frame.
        Geometry::Magnifier { .. } => append_magnifier(snapshot, object, None),
    }

    if shadowed {
        snapshot.pop();
    }
}

/// `spec/05` §5.3 item 4: **one** mask layer for the union of every spotlight.
///
/// Called once per spotlight object and drawn only for the first, because compositing
/// them separately would darken each overlap twice -- two overlapping spotlights have to
/// brighten one region, not punch two holes with a darker seam between them.
fn append_spotlight(snapshot: &gtk::Snapshot, scene: &Scene) {
    let Some(union) = scene.spotlight_union() else { return };
    let order = scene.render_list();
    let Some(first_object) =
        order.iter().find(|o| matches!(o.geometry, Geometry::Spotlight { .. }))
    else {
        return;
    };
    let Geometry::Spotlight { opacity, .. } = first_object.geometry else { return };

    // `push_mask(InvertedAlpha)`: the mask's *transparent* parts are what shows through,
    // so the shapes are the holes and the scrim is what is left. `spec/05` §6 names this
    // node for exactly this reason -- the alternative is four rectangles around each
    // shape, which cannot express an ellipse at all.
    snapshot.push_mask(gsk::MaskMode::InvertedAlpha);
    for object in &order {
        let Geometry::Spotlight { shape, bounds, .. } = &object.geometry else { continue };
        let path = match shape {
            SpotlightShape::Ellipse => ellipse_path(*bounds),
            SpotlightShape::Rounded => rounded_rect_path(*bounds, SpotlightShape::ROUNDED_RADIUS),
            SpotlightShape::Rectangle => rect_path(*bounds),
        };
        snapshot.append_fill(&path, gsk::FillRule::Winding, &gdk::RGBA::BLACK);
    }
    snapshot.pop();

    #[allow(clippy::cast_possible_truncation)]
    let scrim = gdk::RGBA::new(0.0, 0.0, 0.0, opacity as f32);
    // Over the whole canvas, not over the union: the point is that everything *else* is
    // darkened, and a scrim the size of the shapes would darken nothing. The canvas in the
    // picture's coordinates, which this is drawn in: the canvas's own rect missed a picture
    // a background had moved, and over a crop darkened almost none of it (D189).
    snapshot.append_color(&scrim, &to_rect(scene.canvas_in_picture().union(union)));
    snapshot.pop();
}

/// A redaction: its raster, or its **preview** (`spec/05` §6).
///
/// > on drag, preview with a `blur_node` over a clipped base texture (Blur) or a
/// > nearest-filtered `texture_scale_node` (Pixelate) -- pure GPU; on gesture end,
/// > rasterize the exact region on a worker thread … and swap it in.
///
/// The raster is one texture node and is what the export walks (`redact.rs`). The
/// preview is the base image under a blur node, and the clip goes *inside* the blur as
/// well as outside it: a blur node renders its child to an offscreen the size of the
/// child, and the child is a 5K texture -- clipping only the result would have GSK blur
/// fourteen megapixels per frame to show a hundred thousand of them. Pixelate previews
/// as a blur of half a block: GSK has no pixelate node and a two-pass down-and-up scale
/// needs an intermediate texture, which is exactly the raster the worker is making. A
/// black-out is exact as drawn -- `RedactStyle::preview_is_exact` says which.
fn append_redaction(
    snapshot: &gtk::Snapshot,
    bounds: Bounds,
    style: RedactStyle,
    intensity: f64,
    scene: &Scene,
    base: Option<&gdk::Texture>,
    raster: Option<&Raster>,
) {
    if let Some(raster) = raster {
        snapshot.append_texture(&raster.texture, &to_rect(raster.bounds));
        return;
    }
    let rect = to_rect(bounds);
    if style == RedactStyle::BlackOut {
        snapshot.append_color(&gdk::RGBA::BLACK, &rect);
        return;
    }
    let Some(base) = base else {
        // No picture to blur: an opaque stand-in, because a redaction that shows what it
        // is redacting is worse than one that is the wrong shade of grey.
        #[allow(clippy::cast_possible_truncation)]
        let grey = (0.35 + 0.25 * intensity.clamp(0.0, 1.0)) as f32;
        snapshot.append_color(&gdk::RGBA::new(grey, grey, grey, 1.0), &rect);
        return;
    };
    // In document units, which is what the snapshot's transform is in here.
    let radius = match style {
        RedactStyle::Pixelate => ops::block_size(intensity, 1.0) / 2.0,
        RedactStyle::Blur => ops::blur_radius(intensity, 1.0),
        RedactStyle::SecureBlur => ops::secure_cell(intensity, 1.0),
        RedactStyle::BlackOut => 0.0,
    };
    snapshot.push_clip(&rect);
    snapshot.push_blur(radius);
    snapshot.push_clip(&to_rect(bounds.normalised().inflated(radius)));
    snapshot.append_texture(base, &to_rect(scene.base.bounds()));
    snapshot.pop();
    snapshot.pop();
    snapshot.pop();
}

fn append_rect(
    snapshot: &gtk::Snapshot,
    bounds: Bounds,
    filled: bool,
    radius: f64,
    width: f64,
    color: &gdk::RGBA,
) {
    let path = if radius > 0.0 { rounded_rect_path(bounds, radius) } else { rect_path(bounds) };
    if filled {
        snapshot.append_fill(&path, gsk::FillRule::Winding, color);
    } else {
        stroke(snapshot, &path, width, color);
    }
}

/// `spec/05` §4.2's four styles.
///
/// Three things the first version got wrong, all visible on the reference canvas
/// (`editor-arrow-styles-on-canvas.png`) and reported from hardware as "arrows don't look
/// right":
///
/// - **The shaft ran to the tip.** A round-capped stroke ending at the tip pokes half a
///   stroke out past the point of the triangle drawn over it, so every arrow had a blunt
///   nose. The shaft now stops at the head's base and the cap disappears inside it.
/// - **The head was proportional with no floor**, so the default weight got a nine-pixel
///   head and level 1 a three-pixel one. `style::arrowhead_length` says why.
/// - **Fancy was a wedge the width of the stroke**, which at level 3 is a three-pixel
///   sliver. On the reference it is the *heaviest* style: a shaft that is a point at the
///   tail and wider than the stroke at the head, under a swept head with a notched back.
#[allow(clippy::too_many_arguments)]
fn append_arrow(
    snapshot: &gtk::Snapshot,
    start: Point,
    end: Point,
    ctrl: Option<Point>,
    style: octosnap_scene::ArrowStyle,
    head_kind: ArrowHead,
    width: f64,
    color: &gdk::RGBA,
) {
    use octosnap_scene::ArrowStyle as A;

    let chord = start.distance_to(end);
    if chord == 0.0 {
        return;
    }
    // Never longer than a share of the arrow itself: a 20-unit arrow with a 30-unit head
    // is a triangle with a dot behind it.
    let head = arrowhead_length(width).min(chord * 0.45);
    // How far the shaft stops short of each tip, which depends on the head's shape: an
    // open chevron hides nothing, a disc hides half of itself.
    let depth = head * head_kind.shaft_depth();
    let double = matches!(style, A::DoubleHeaded);
    // Both Curved *and* Double arc. `spec/05` §4.2 corrects itself on precisely this
    // point -- "**Double**: **also curved**" -- and `scene::tool::bow` hands both a
    // control point, so the only thing needed here is to use it.
    let curved = matches!(style, A::Curved | A::DoubleHeaded);

    match ctrl.filter(|_| curved) {
        Some(c) => {
            // The shaft stops short of each head it carries, by the head's depth: the
            // round cap then ends inside the triangle instead of at its tip. On a curve
            // that is a trim of the *parameter*, worked out from the curve's speed at
            // the end (`trim_parameter`).
            let from = if double { trim_parameter(c, start, depth) } else { 0.0 };
            let to = 1.0 - trim_parameter(c, end, depth);
            if to > from {
                let (p0, p1, p2) = quad_sub(start, c, end, from, to);
                stroke(snapshot, &quad_path(p0, p1, p2), width, color);
            }
            // Each head points along the curve's tangent at its end, which for a
            // quadratic is the line from the control point -- aiming it along the chord
            // instead puts a bowed arrow's head off the curve.
            append_head(snapshot, head_kind, end, unit(c, end), head, width, color);
            if double {
                append_head(snapshot, head_kind, start, unit(c, start), head, width, color);
            }
        }
        None => {
            let along = unit(start, end);
            if matches!(style, A::Fancy) {
                // A tapered shaft cannot be a stroke of a line -- a stroke has one width
                // -- so Fancy is a filled quadrilateral: a point at the tail, opening to
                // more than the stroke where it runs into the head. It ends *inside* the
                // head -- a share of the way past where a stroked shaft would stop -- so
                // the join is under the solid part of whichever head is on.
                let deep = along_by(end, along, -(depth * 0.85).max(width));
                snapshot.append_fill(
                    &tapered_shaft(start, deep, along, width),
                    gsk::FillRule::Winding,
                    color,
                );
            } else {
                let shaft_start = if double { along_by(start, along, depth) } else { start };
                let shaft_end = along_by(end, along, -depth);
                if shaft_start.distance_to(shaft_end) > 0.0
                    && shaft_start.distance_to(end) > shaft_end.distance_to(end)
                {
                    stroke(snapshot, &segment_path(shaft_start, shaft_end), width, color);
                }
            }
            append_head(snapshot, head_kind, end, along, head, width, color);
            if double {
                append_head(snapshot, head_kind, start, (-along.0, -along.1), head, width, color);
            }
        }
    }
}

/// One arrowhead with its tip at `tip`, pointing along the unit vector `along`.
///
/// The triangle is `spec/05` §4.2's -- half as wide as it is long, `1.6w` against
/// `3.2w`. The other four are the user's ("give different options like 5 most common"):
/// a swept head with its back edges drawn in to a notch, an open chevron of two strokes,
/// a diamond, and a disc. Every one has its tip at `tip`, so switching heads never moves
/// the arrow.
fn append_head(
    snapshot: &gtk::Snapshot,
    kind: ArrowHead,
    tip: Point,
    along: (f64, f64),
    head: f64,
    width: f64,
    color: &gdk::RGBA,
) {
    let perp = (-along.1, along.0);
    let at = |depth: f64, side: f64| {
        let p = along_by(tip, along, -depth);
        #[allow(clippy::cast_possible_truncation)]
        graphene::Point::new((p.x + perp.0 * side) as f32, (p.y + perp.1 * side) as f32)
    };
    let builder = gsk::PathBuilder::new();
    let move_to = |b: &gsk::PathBuilder, p: graphene::Point| b.move_to(p.x(), p.y());
    let line_to = |b: &gsk::PathBuilder, p: graphene::Point| b.line_to(p.x(), p.y());
    match kind {
        ArrowHead::Triangle => {
            move_to(&builder, at(0.0, 0.0));
            line_to(&builder, at(head, head * 0.5));
            line_to(&builder, at(head, -head * 0.5));
            builder.close();
            snapshot.append_fill(&builder.to_path(), gsk::FillRule::Winding, color);
        }
        ArrowHead::Swept => {
            // A fifth longer than the triangle as well as wider: the heaviest head, as
            // the reference's Fancy arrows wear it.
            let head = head * 1.2;
            move_to(&builder, at(0.0, 0.0));
            line_to(&builder, at(head, head * 0.55));
            line_to(&builder, at(head * 0.7, 0.0));
            line_to(&builder, at(head, -head * 0.55));
            builder.close();
            snapshot.append_fill(&builder.to_path(), gsk::FillRule::Winding, color);
        }
        ArrowHead::Open => {
            // Two strokes of the shaft's own width, so the chevron is as heavy as the
            // line it ends -- and the shaft runs to the apex (`shaft_depth` is zero).
            move_to(&builder, at(head, head * 0.5));
            line_to(&builder, at(0.0, 0.0));
            line_to(&builder, at(head, -head * 0.5));
            stroke(snapshot, &builder.to_path(), width, color);
        }
        ArrowHead::Diamond => {
            move_to(&builder, at(0.0, 0.0));
            line_to(&builder, at(head * 0.5, head * 0.4));
            line_to(&builder, at(head, 0.0));
            line_to(&builder, at(head * 0.5, -head * 0.4));
            builder.close();
            snapshot.append_fill(&builder.to_path(), gsk::FillRule::Winding, color);
        }
        ArrowHead::Dot => {
            // The disc's far edge is the tip, its centre is where the shaft stops.
            let radius = head * ArrowHead::Dot.shaft_depth();
            #[allow(clippy::cast_possible_truncation)]
            builder.add_circle(&at(radius, 0.0), radius as f32);
            snapshot.append_fill(&builder.to_path(), gsk::FillRule::Winding, color);
        }
    }
}

/// `spec/05` §4.2's Fancy shaft: a point at the tail, `half` wide where it meets the head.
fn tapered_shaft(tail: Point, head_end: Point, along: (f64, f64), half: f64) -> gsk::Path {
    let perp = (-along.1, along.0);
    let builder = gsk::PathBuilder::new();
    #[allow(clippy::cast_possible_truncation)]
    {
        builder.move_to(tail.x as f32, tail.y as f32);
        builder.line_to((head_end.x + perp.0 * half) as f32, (head_end.y + perp.1 * half) as f32);
        builder.line_to((head_end.x - perp.0 * half) as f32, (head_end.y - perp.1 * half) as f32);
    }
    builder.close();
    builder.to_path()
}

/// The unit vector from `from` towards `to`; along `+x` if the two coincide.
fn unit(from: Point, to: Point) -> (f64, f64) {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = dx.hypot(dy);
    if length == 0.0 { (1.0, 0.0) } else { (dx / length, dy / length) }
}

fn along_by(point: Point, along: (f64, f64), distance: f64) -> Point {
    Point::new(point.x + along.0 * distance, point.y + along.1 * distance)
}

/// How much of a quadratic's parameter covers `distance` of arc at the end nearest
/// `endpoint`, whose neighbouring control point is `ctrl`.
///
/// The curve's speed at either end is twice the distance to the control point, so this is
/// a first-order estimate -- and a first-order estimate is enough, because it is trimming
/// a shaft under an opaque head where a unit or two either way cannot be seen.
fn trim_parameter(ctrl: Point, endpoint: Point, distance: f64) -> f64 {
    let speed = 2.0 * ctrl.distance_to(endpoint);
    if speed == 0.0 { 0.0 } else { (distance / speed).min(0.45) }
}

/// The control points of the piece of a quadratic between parameters `a` and `b`.
///
/// The end points are the curve at `a` and `b`; the middle one is the blend that keeps
/// the piece on the same parabola -- `(1-a)(1-b) p0 + (a+b-2ab) p1 + ab p2`, which
/// reduces to de Casteljau's `lerp(p0, p1, b)` when `a` is zero.
fn quad_sub(p0: Point, p1: Point, p2: Point, a: f64, b: f64) -> (Point, Point, Point) {
    let at = |t: f64| {
        let (u, v) = (1.0 - t, t);
        Point::new(
            u * u * p0.x + 2.0 * u * v * p1.x + v * v * p2.x,
            u * u * p0.y + 2.0 * u * v * p1.y + v * v * p2.y,
        )
    };
    let middle = Point::new(
        (1.0 - a) * (1.0 - b) * p0.x + (a + b - 2.0 * a * b) * p1.x + a * b * p2.x,
        (1.0 - a) * (1.0 - b) * p0.y + (a + b - 2.0 * a * b) * p1.y + a * b * p2.y,
    );
    (at(a), middle, at(b))
}

#[allow(clippy::cast_possible_truncation)]
fn quad_path(p0: Point, p1: Point, p2: Point) -> gsk::Path {
    let builder = gsk::PathBuilder::new();
    builder.move_to(p0.x as f32, p0.y as f32);
    builder.quad_to(p1.x as f32, p1.y as f32, p2.x as f32, p2.y as f32);
    builder.to_path()
}

/// `spec/05` §4.6 and §4.7: the pencil and the highlighter are one geometry.
///
/// The difference is the blend mode, which is why `Geometry::Path` carries a
/// `highlighter` flag rather than there being two object types: a highlighter stroke is a
/// pencil stroke that multiplies, and `spec/05` §5.3 lists it in the same group.
fn append_freehand(
    snapshot: &gtk::Snapshot,
    points: &[Point],
    smoothing: bool,
    highlighter: bool,
    band: Option<f64>,
    width: f64,
    color: &gdk::RGBA,
) {
    if points.is_empty() {
        return;
    }
    let path = if smoothing { smooth_path(points) } else { polyline_path(points) };

    // `spec/05` §4.7: "Like Pencil but with a flat wide stroke (`3 x size`), square caps,
    // multiply blend, colour alpha 55 %." Three of the four are here; the fourth is not,
    // and the reason is worth writing down because the first version of this function
    // looked like it had all four.
    //
    // `push_blend(Multiply)` blends **its own two children** -- GTK's own words: "Until
    // the first call to pop, the bottom image for the blend operation will be recorded.
    // After that call, the top image ... Calling this function requires two subsequent
    // calls to pop." So a push around one stroke, with a single pop, made the stroke the
    // *bottom* child and blended it against an empty top, and left the node tree
    // unbalanced. It also could never have worked as intended: what a highlighter has to
    // multiply against is everything already drawn beneath it, and a snapshot cannot hand
    // its own accumulated content back as a child.
    //
    // Doing it properly means the scene below the highlighter run becomes the bottom
    // child and the strokes become the top -- a restructure of `append_scene`, not a push
    // around a stroke. Until then the 55 % alpha carries it, which is the part of §4.7
    // that a user actually sees: the wash is translucent and dark text stays readable
    // under it. The alpha is stored on the object by `scene::tool`, so the export gets
    // the same thing the canvas shows -- and when the blend arrives it will change both
    // at once, which is the property §6 is protecting.
    // §4.7's `3 x size`, unless the smart highlighter snapped this stroke to a line of
    // text -- and then it is as tall as the line, which is the whole point of snapping.
    #[allow(clippy::cast_possible_truncation)]
    let stroke_spec = gsk::Stroke::new(if highlighter {
        octosnap_scene::tool::highlighter_width(width, band) as f32
    } else {
        width as f32
    });
    if highlighter {
        // Square caps and mitre joins: a marker has a chisel nib, and §4.7 asks for the
        // flat end that gives a highlighted line its blunt start and finish.
        stroke_spec.set_line_cap(gsk::LineCap::Square);
        stroke_spec.set_line_join(gsk::LineJoin::Miter);
    } else {
        // Round caps and joins for the pencil: a butt cap on a freehand stroke shows
        // every segment boundary as a notch.
        stroke_spec.set_line_cap(gsk::LineCap::Round);
        stroke_spec.set_line_join(gsk::LineJoin::Round);
    }
    snapshot.append_stroke(&path, &stroke_spec, color);
}

/// `ANN-34`'s pen: a stroke as wide as it was pressed, which is an area and not a line of
/// one width (`octosnap_scene::pressure`). Answers whether it drew one: a stroke with no
/// pressure is [`append_freehand`]'s.
fn append_pressed(
    snapshot: &gtk::Snapshot,
    points: &[Point],
    smoothing: bool,
    pressure: Option<&[f32]>,
    width: f64,
    color: &gdk::RGBA,
) -> bool {
    let Some(pressure) = pressure else { return false };
    let pieces = octosnap_scene::pressure::pieces(points, pressure, width, smoothing);
    if pieces.is_empty() {
        return false;
    }
    snapshot.append_fill(&pressed_path(&pieces), gsk::FillRule::Winding, color);
    true
}

/// A pressed stroke's discs and bands as one path, each a contour and all of them
/// clockwise, so a winding fill covers where they overlap once.
#[allow(clippy::cast_possible_truncation)]
fn pressed_path(pieces: &[octosnap_scene::pressure::Piece]) -> gsk::Path {
    use octosnap_scene::pressure::Piece;
    let builder = gsk::PathBuilder::new();
    let quarter = std::f32::consts::FRAC_1_SQRT_2;
    for piece in pieces {
        match *piece {
            // Four quarter circles, clockwise on the screen from the right.
            Piece::Disc { center, radius } => {
                let (x, y, r) = (center.x as f32, center.y as f32, radius as f32);
                builder.move_to(x + r, y);
                builder.conic_to(x + r, y + r, x, y + r, quarter);
                builder.conic_to(x - r, y + r, x - r, y, quarter);
                builder.conic_to(x - r, y - r, x, y - r, quarter);
                builder.conic_to(x + r, y - r, x + r, y, quarter);
                builder.close();
            }
            Piece::Band(corners) => {
                builder.move_to(corners[0].x as f32, corners[0].y as f32);
                for corner in &corners[1..] {
                    builder.line_to(corner.x as f32, corner.y as f32);
                }
                builder.close();
            }
        }
    }
    builder.to_path()
}

/// `spec/05` §4.9's numbered badge. Always on top, which the scene's order already did.
fn append_counter(
    snapshot: &gtk::Snapshot,
    center: Point,
    number: u32,
    style: octosnap_scene::CounterStyle,
    radius: f64,
    tail: Option<Point>,
    color: &gdk::RGBA,
) {
    // `ANN-29`'s tail first, so the disc covers the wedge's base (`callout::badge_wedge`).
    if let Some(wedge) =
        tail.and_then(|tip| octosnap_scene::callout::badge_wedge(center, radius, tip))
    {
        append_wedge(snapshot, wedge, color);
    }
    // Always filled. `spec/05` §4.9: "Click places a **filled** circular badge … drawn in
    // the current colour with a white glyph", and its [P→V] correction says there is no
    // Filled/Outline choice at all -- the four options are numbering systems. This
    // function used to branch on a variant the UI would never offer.
    let bounds = Bounds::new(center.x - radius, center.y - radius, radius * 2.0, radius * 2.0);
    snapshot.append_fill(&ellipse_path(bounds), gsk::FillRule::Winding, color);
    // The glyph is a Pango layout, so it belongs with the canvas -- see the note on
    // `Geometry::Text` above. `Canvas::draw_counter_glyphs` draws it, and the style
    // decides what it says.
    let _ = (style, number);
}

/// `ANN-29`'s wedge: a tail's triangle, filled.
#[allow(clippy::cast_possible_truncation)]
pub fn append_wedge(snapshot: &gtk::Snapshot, wedge: [Point; 3], color: &gdk::RGBA) {
    let builder = gsk::PathBuilder::new();
    builder.move_to(wedge[0].x as f32, wedge[0].y as f32);
    builder.line_to(wedge[1].x as f32, wedge[1].y as f32);
    builder.line_to(wedge[2].x as f32, wedge[2].y as f32);
    builder.close();
    snapshot.append_fill(&builder.to_path(), gsk::FillRule::Winding, color);
}

/// `ANN-29`'s tail on a label with no plate: a line, round at both ends, and a dot on the
/// tip.
pub fn append_pointer_line(
    snapshot: &gtk::Snapshot,
    line: octosnap_scene::Segment,
    width: f64,
    dot: f64,
    color: &gdk::RGBA,
) {
    stroke(snapshot, &segment_path(line.start, line.end), width, color);
    let tip = line.end;
    let disc = Bounds::new(tip.x - dot, tip.y - dot, dot * 2.0, dot * 2.0);
    snapshot.append_fill(&ellipse_path(disc), gsk::FillRule::Winding, color);
}

// --- paths ---------------------------------------------------------------------------

fn stroke(snapshot: &gtk::Snapshot, path: &gsk::Path, width: f64, color: &gdk::RGBA) {
    #[allow(clippy::cast_possible_truncation)]
    let spec = gsk::Stroke::new(width as f32);
    spec.set_line_cap(gsk::LineCap::Round);
    spec.set_line_join(gsk::LineJoin::Round);
    snapshot.append_stroke(path, &spec, color);
}

#[allow(clippy::cast_possible_truncation)]
fn segment_path(start: Point, end: Point) -> gsk::Path {
    let builder = gsk::PathBuilder::new();
    builder.move_to(start.x as f32, start.y as f32);
    builder.line_to(end.x as f32, end.y as f32);
    builder.to_path()
}

#[allow(clippy::cast_possible_truncation)]
fn rect_path(bounds: Bounds) -> gsk::Path {
    let builder = gsk::PathBuilder::new();
    builder.add_rect(&to_rect(bounds));
    builder.to_path()
}

#[allow(clippy::cast_possible_truncation)]
fn rounded_rect_path(bounds: Bounds, radius: f64) -> gsk::Path {
    let r = bounds.normalised();
    // Clamped to half the shorter side, or a radius larger than the rect turns it inside
    // out -- which GSK will happily draw.
    let radius = radius.min(r.width / 2.0).min(r.height / 2.0).max(0.0) as f32;
    let rounded = gsk::RoundedRect::from_rect(to_rect(r), radius);
    let builder = gsk::PathBuilder::new();
    builder.add_rounded_rect(&rounded);
    builder.to_path()
}

#[allow(clippy::cast_possible_truncation)]
fn ellipse_path(bounds: Bounds) -> gsk::Path {
    let r = bounds.normalised();
    let builder = gsk::PathBuilder::new();
    // A circle is a rounded rect whose radius is half its side, which is exact for the
    // ellipse inscribed in these bounds and needs no bezier approximation of our own.
    let rounded = gsk::RoundedRect::new(
        to_rect(r),
        graphene::Size::new(r.width as f32 / 2.0, r.height as f32 / 2.0),
        graphene::Size::new(r.width as f32 / 2.0, r.height as f32 / 2.0),
        graphene::Size::new(r.width as f32 / 2.0, r.height as f32 / 2.0),
        graphene::Size::new(r.width as f32 / 2.0, r.height as f32 / 2.0),
    );
    builder.add_rounded_rect(&rounded);
    builder.to_path()
}

#[allow(clippy::cast_possible_truncation)]
fn polyline_path(points: &[Point]) -> gsk::Path {
    let builder = gsk::PathBuilder::new();
    let Some(first) = points.first() else { return builder.to_path() };
    builder.move_to(first.x as f32, first.y as f32);
    for p in &points[1..] {
        builder.line_to(p.x as f32, p.y as f32);
    }
    // A single point is a dot, and a path with only a `move_to` draws nothing at all --
    // so it gets a zero-length line, which a round cap turns into the dot it should be.
    if points.len() == 1 {
        builder.line_to(first.x as f32, first.y as f32);
    }
    builder.to_path()
}

/// `spec/05` §4.6's "auto-smoothing", as a quadratic through the midpoints.
///
/// Each segment's midpoint is an on-curve point and each original sample is the control
/// point between two of them, which is the standard trick: it passes through no sample
/// exactly but stays within half a sample of every one of them, needs no tangent
/// estimation, and cannot overshoot -- so a fast scribble does not develop loops the user
/// did not draw. A Catmull-Rom through the samples themselves would be more faithful and
/// does overshoot.
#[allow(clippy::cast_possible_truncation)]
fn smooth_path(points: &[Point]) -> gsk::Path {
    if points.len() < 3 {
        return polyline_path(points);
    }
    let builder = gsk::PathBuilder::new();
    let midpoint = |a: Point, b: Point| Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);

    builder.move_to(points[0].x as f32, points[0].y as f32);
    for window in points.windows(3) {
        let mid = midpoint(window[1], window[2]);
        builder.quad_to(window[1].x as f32, window[1].y as f32, mid.x as f32, mid.y as f32);
    }
    // The last sample is an endpoint, not a control point: a stroke that stopped short of
    // where the pointer was released reads as a dropped input event.
    if let Some(last) = points.last() {
        builder.line_to(last.x as f32, last.y as f32);
    }
    builder.to_path()
}

// --- conversions ---------------------------------------------------------------------

#[allow(clippy::cast_possible_truncation)]
fn to_rect(bounds: Bounds) -> graphene::Rect {
    let r = bounds.normalised();
    graphene::Rect::new(r.x as f32, r.y as f32, r.width as f32, r.height as f32)
}

/// `scene`'s `f64` channels to `gdk`'s `f32`. Narrowing, and exact -- see the note on
/// `Rgba` for why the document holds the wider type.
#[allow(clippy::cast_possible_truncation)]
fn to_rgba(color: Rgba) -> gdk::RGBA {
    gdk::RGBA::new(color.r as f32, color.g as f32, color.b as f32, color.a as f32)
}

/// `spec/05` §5.1's `shadow: true`, sized off the stroke so a heavy object casts a
/// heavier shadow. Not in `spec/09` §3's table, which is about motion.
#[allow(clippy::cast_possible_truncation)]
fn shadow_for(width: f64) -> gsk::Shadow {
    gsk::Shadow::new(
        gdk::RGBA::new(0.0, 0.0, 0.0, 0.45),
        0.0,
        (width * 0.4) as f32,
        (width * 0.8) as f32,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use octosnap_scene::style::Style;

    /// A `width` x `height` picture, red left of `blue_from` and blue from there on.
    fn picture(width: usize, height: usize, blue_from: usize) -> gdk::Texture {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _ in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(if x < blue_from {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        let bytes = gtk::glib::Bytes::from_owned(pixels);
        gdk::MemoryTexture::new(
            i32::try_from(width).unwrap(),
            i32::try_from(height).unwrap(),
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            width * 4,
        )
        .upcast()
    }

    /// A lens of 40 at twice: its spot is 20 across either way of `source`.
    fn lens(source: Point) -> Object {
        Object::new(
            1,
            0,
            Style::default(),
            Geometry::Magnifier {
                center: Point::new(300.0, 100.0),
                radius: 40.0,
                source,
                zoom: 2.0,
            },
        )
    }

    /// `ANN-32`: in an SVG a lens is its spot's pixels alone, one image at their own size --
    /// not the picture again, and not cairo's 300 dpi stand-ins for a texture node's padded
    /// edges.
    #[test]
    fn a_lens_in_an_svg_is_its_spots_pixels_alone() {
        let picture = picture(400, 200, 200);
        let canvas = Bounds::new(0.0, 0.0, 400.0, 200.0);
        let spot = lens(Point::new(100.0, 100.0));
        let node = flattened_spot(&spot, &picture, canvas, 1.0, Placement::NONE).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lens.svg");
        let surface = cairo::SvgSurface::new(400.0, 200.0, Some(&path)).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        node.draw(&cr);
        drop(cr);
        surface.finish();
        let svg = std::fs::read_to_string(&path).unwrap();
        // The spot is 40 across, and the crop a pixel more on each side.
        assert_eq!(svg.matches("<image").count(), 1, "{svg}");
        assert!(svg.contains(r#"width="42""#) && svg.contains(r#"height="42""#), "{svg}");
    }

    /// The flattened picture is the canvas's, and a background's margin and inset put the
    /// picture somewhere inside it: the spot is cut from there, and put back where the lens
    /// looks for it.
    #[test]
    fn a_lens_takes_its_spot_from_where_the_picture_sits_on_the_canvas() {
        // The canvas is blue from 200 on, and the picture sits at 150, 50 at half its size:
        // its own 150 is the canvas's 225, blue, and its own 60 the canvas's 180, red. Read
        // as if the picture filled the canvas, both would be red.
        let picture = picture(400, 200, 200);
        let canvas = Bounds::new(0.0, 0.0, 400.0, 200.0);
        let place = Placement { x: 150.0, y: 50.0, scale: 0.5 };
        let under = |source: Point| {
            let node = flattened_spot(&lens(source), &picture, canvas, 1.0, place).unwrap();
            let mut image = cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 200).unwrap();
            {
                let cr = cairo::Context::new(&image).unwrap();
                node.draw(&cr);
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let at = source.y as usize * usize::try_from(image.stride()).unwrap()
                + source.x as usize * 4;
            let data = image.data().unwrap();
            u32::from_ne_bytes(data[at..at + 4].try_into().unwrap())
        };
        assert_eq!(under(Point::new(150.0, 100.0)), 0xff00_00ff, "blue");
        assert_eq!(under(Point::new(60.0, 100.0)), 0xffff_0000, "red");
    }

    /// D189: a background over the right half of a picture red on the left and blue on the
    /// right, with a green margin of 20, at square corners and round, with and without a
    /// spotlight at full opacity on the picture's top-left. The margin is green, never the
    /// red beyond the crop, and the spotlight blacks out the picture but for its shape.
    ///
    /// Drawn as the export draws it, through a `gtk::Snapshot`, which needs a display. In a
    /// nested shell, with `nested-env.sh` sourced: `cargo test -p octosnap-app --bins --
    /// --ignored a_background_over_a_crop`.
    #[gtk::test]
    #[ignore = "draws through GTK, which needs a display"]
    fn a_background_over_a_crop_shows_its_margin_and_its_spotlight_darkens_the_picture() {
        use octosnap_scene::background::{Background, BackgroundParams};
        use octosnap_scene::{Base, Scene, layout};
        let texture = picture(200, 100, 100);
        for (corner, spot) in [(0.0, false), (0.08, false), (0.0, true), (0.08, true)] {
            let mut scene = Scene::new(Base::new("base.png", 200.0, 100.0, 1.0));
            let source = Bounds::new(100.0, 0.0, 100.0, 100.0);
            let params = BackgroundParams {
                padding: 20.0,
                corner_radius: corner,
                shadow_intensity: 0.0,
                background: Background::Color { color: Rgba::new(0.0, 1.0, 0.0, 1.0) },
                ..BackgroundParams::default()
            };
            scene.canvas = layout(source, &params).canvas;
            scene.add(Object::new(
                0,
                0,
                Style::default(),
                Geometry::Background { params, source: Some(source), fill: None },
            ));
            if spot {
                scene.add(Object::new(
                    1,
                    0,
                    Style::default(),
                    Geometry::Spotlight {
                        shape: SpotlightShape::Rectangle,
                        bounds: Bounds::new(110.0, 10.0, 20.0, 20.0),
                        opacity: 1.0,
                    },
                ));
            }
            let snapshot = gtk::Snapshot::new();
            let draw = |s: &gtk::Snapshot, o: &Object| {
                append_object(s, o, &scene, Some(&texture), None, None);
            };
            append_scene(&snapshot, &scene, Some(&texture), &draw, Part::All, Lens::Drawn);
            let node = snapshot.to_node().unwrap();
            let mut image = cairo::ImageSurface::create(cairo::Format::ARgb32, 140, 140).unwrap();
            {
                let cr = cairo::Context::new(&image).unwrap();
                node.draw(&cr);
            }
            let stride = usize::try_from(image.stride()).unwrap();
            let data = image.data().unwrap();
            let at = |x: usize, y: usize| {
                let i = y * stride + x * 4;
                u32::from_ne_bytes(data[i..i + 4].try_into().unwrap())
            };
            let (green, blue, black) = (0xff00_ff00, 0xff00_00ff, 0xff00_0000);
            let case = format!("corners {corner}, spotlight {spot}");
            // The canvas is 140 across, the picture at (20, 20), 100 across; the spotlight's
            // shape is at (30, 30) on the canvas, 20 across.
            assert_eq!(at(5, 70), green, "the left margin, {case}");
            assert_eq!(at(135, 70), green, "the right margin, {case}");
            assert_eq!(at(35, 35), blue, "in the spotlight's shape, {case}");
            let rest = if spot { black } else { blue };
            assert_eq!(at(70, 70), rest, "the picture's middle, {case}");
            assert_eq!(at(115, 115), rest, "the picture's far corner, {case}");
        }
    }

    /// D191: the fill a crop beyond the image leaves frames nothing. A blue capture 100 by
    /// 100 was cropped 50 wider on its right and filled green, and a red picture put on the
    /// area that added. It shows there, and a spotlight at full opacity darkens that area as
    /// it does the picture. Framed by the base image's rect, as every background was once
    /// D189 clipped at square corners, the picture was clipped away and the area stayed
    /// green under the scrim.
    ///
    /// In a nested shell, as the test above: `--ignored the_crop_s_own_fill`.
    #[gtk::test]
    #[ignore = "draws through GTK, which needs a display"]
    fn the_crop_s_own_fill_frames_nothing() {
        use octosnap_scene::background::BackgroundParams;
        use octosnap_scene::{Base, Scene};
        let (blue_picture, red_picture) = (picture(100, 100, 0), picture(10, 10, 10));
        for spot in [false, true] {
            let mut scene = Scene::new(Base::new("base.png", 100.0, 100.0, 1.0));
            scene.canvas = Bounds::new(0.0, 0.0, 150.0, 100.0);
            let green = Rgba::new(0.0, 1.0, 0.0, 1.0);
            let geometry = Geometry::Background {
                params: BackgroundParams::plain(green),
                source: None,
                fill: None,
            };
            scene.add(Object::new(-1, 0, Style::new(green, 1, false), geometry));
            let bounds = Bounds::new(110.0, 10.0, 30.0, 30.0);
            let file = "assets/red.png".to_owned();
            scene.add(Object::new(1, 0, Style::default(), Geometry::Image { bounds, file }));
            if spot {
                let shape = SpotlightShape::Rectangle;
                let bounds = Bounds::new(10.0, 10.0, 20.0, 20.0);
                let geometry = Geometry::Spotlight { shape, bounds, opacity: 1.0 };
                scene.add(Object::new(2, 0, Style::default(), geometry));
            }
            let snapshot = gtk::Snapshot::new();
            let draw = |s: &gtk::Snapshot, o: &Object| {
                append_object(s, o, &scene, Some(&blue_picture), None, Some(&red_picture));
            };
            append_scene(&snapshot, &scene, Some(&blue_picture), &draw, Part::All, Lens::Drawn);
            let node = snapshot.to_node().unwrap();
            let mut image = cairo::ImageSurface::create(cairo::Format::ARgb32, 150, 100).unwrap();
            {
                let cr = cairo::Context::new(&image).unwrap();
                node.draw(&cr);
            }
            let stride = usize::try_from(image.stride()).unwrap();
            let data = image.data().unwrap();
            let at = |x: usize, y: usize| {
                let i = y * stride + x * 4;
                u32::from_ne_bytes(data[i..i + 4].try_into().unwrap())
            };
            let (green, red, black) = (0xff00_ff00, 0xffff_0000, 0xff00_0000);
            let case = format!("spotlight {spot}");
            assert_eq!(
                at(125, 25),
                if spot { black } else { red },
                "the picture put there, {case}"
            );
            assert_eq!(at(125, 80), if spot { black } else { green }, "the area added, {case}");
        }
    }

    /// D192: the panel's background over the crop's own fill frames the whole crop, the area
    /// it added included, and keeps the fill there. D191's document, a blue capture 100 by 100
    /// cropped 50 wider and filled green with a red picture on the added area, given a yellow
    /// background at a margin of 20 through the panel's edit, at square corners and round.
    /// Laid out over the capture's rect, as it was, the canvas was 140 across and the red
    /// picture was clipped away.
    ///
    /// In a nested shell, as the tests above: `--ignored the_panel_s_background_over`.
    #[gtk::test]
    #[ignore = "draws through GTK, which needs a display"]
    fn the_panel_s_background_over_the_crop_s_fill_frames_the_whole_crop() {
        use octosnap_scene::background::{self, Background, BackgroundParams};
        use octosnap_scene::{Base, Command, History, Scene};
        let (blue_picture, red_picture) = (picture(100, 100, 0), picture(10, 10, 10));
        for corners in [0.0, 0.08] {
            let mut scene = Scene::new(Base::new("base.png", 100.0, 100.0, 1.0));
            scene.canvas = Bounds::new(0.0, 0.0, 150.0, 100.0);
            let green = Rgba::new(0.0, 1.0, 0.0, 1.0);
            let geometry = Geometry::Background {
                params: BackgroundParams::plain(green),
                source: None,
                fill: None,
            };
            scene.add(Object::new(-1, 0, Style::new(green, 1, false), geometry));
            let bounds = Bounds::new(110.0, 10.0, 30.0, 30.0);
            let file = "assets/red.png".to_owned();
            scene.add(Object::new(1, 0, Style::default(), Geometry::Image { bounds, file }));
            let params = BackgroundParams {
                background: Background::Color { color: Rgba::new(1.0, 1.0, 0.0, 1.0) },
                padding: 20.0,
                corner_radius: corners,
                shadow_intensity: 0.0,
                ..BackgroundParams::default()
            };
            let commands = background::apply(&scene, &params, scene.picture_rect(), 0);
            assert!(History::new().apply(&mut scene, Command::Batch(commands)));
            assert_eq!(scene.canvas, Bounds::new(0.0, 0.0, 190.0, 140.0));
            let snapshot = gtk::Snapshot::new();
            let draw = |s: &gtk::Snapshot, o: &Object| {
                append_object(s, o, &scene, Some(&blue_picture), None, Some(&red_picture));
            };
            append_scene(&snapshot, &scene, Some(&blue_picture), &draw, Part::All, Lens::Drawn);
            let node = snapshot.to_node().unwrap();
            let mut image = cairo::ImageSurface::create(cairo::Format::ARgb32, 190, 140).unwrap();
            {
                let cr = cairo::Context::new(&image).unwrap();
                node.draw(&cr);
            }
            let stride = usize::try_from(image.stride()).unwrap();
            let data = image.data().unwrap();
            let at = |x: usize, y: usize| {
                let i = y * stride + x * 4;
                u32::from_ne_bytes(data[i..i + 4].try_into().unwrap())
            };
            let (yellow, green, red, blue) = (0xffff_ff00, 0xff00_ff00, 0xffff_0000, 0xff00_00ff);
            let case = format!("corners {corners}");
            // The picture is at (20, 20) on the canvas, 150 by 100.
            assert_eq!(at(5, 70), yellow, "the margin, {case}");
            assert_eq!(at(70, 70), blue, "the capture, {case}");
            assert_eq!(at(145, 45), red, "the picture on the area the crop added, {case}");
            assert_eq!(at(145, 100), green, "the area the crop added, {case}");
            // The frame's corner is the whole crop's: round, it shows the margin at its tip.
            assert_eq!(at(168, 21), if corners > 0.0 { yellow } else { green }, "{case}");
            assert_eq!(at(164, 25), green, "inside the frame's corner, {case}");
        }
    }
}
