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
use gtk::{gdk, gsk, graphene};
use octosnap_scene::redact as ops;
use octosnap_scene::style::{ArrowHead, RedactStyle, Rgba, SpotlightShape, arrowhead_length};
use octosnap_scene::{Background, BackgroundParams, Bounds, Geometry, Object, Point, Scene};

use super::redact::Raster;

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
pub fn append_scene(
    snapshot: &gtk::Snapshot,
    scene: &Scene,
    base: Option<&gdk::Texture>,
    draw: &dyn Fn(&gtk::Snapshot, &Object),
) {
    // The base image, under everything except a background object. `spec/05` §5.3 puts
    // the background at group 0 and the base at group 1, and the scene's order already
    // reflects that -- so a background is appended before this and shows around a canvas
    // that a crop has made larger than the capture (§11 item 7).
    // Once, and threaded through everything below it. Asking four times per frame -- twice
    // here and twice inside the spotlight mask -- was an allocation and a sort each time,
    // plus a linear `Scene::get` per id to turn the ids back into objects (D54).
    let order = scene.render_list();

    // The background is drawn in the *canvas's* coordinates, and everything above it in
    // the *picture's*. The two are the same space until `spec/05` §4.13's inset shrinks
    // the picture inside the canvas, and then they are not -- so the transform goes here,
    // around the base image and every object at once, rather than into each of them.
    // Around all of them, because an arrow pointing at a button has to keep pointing at
    // it: the annotations are on the picture, not beside it.
    for object in &order {
        if matches!(object.geometry, Geometry::Background { .. }) {
            draw(snapshot, object);
        }
    }

    // `spec/05` §4.13's rounded corners and shadow are properties of the *picture*, so
    // they wrap it: the shadow is cast by the clip's silhouette, which is why the clip
    // goes inside the shadow and not beside it. An annotation drawn on the picture adds
    // nothing to that silhouette, which is right -- an arrow does not cast its own shadow
    // onto the gradient.
    let frame = picture_frame(scene);
    let mut pushed = 0_u8;
    if let Some((rect, radius, shadow)) = frame {
        if let Some(shadow) = shadow {
            snapshot.push_shadow(&[shadow]);
            pushed += 1;
        }
        if radius > 0.0 {
            snapshot.push_rounded_clip(&rounded(rect, radius));
            pushed += 1;
        }
    }

    let place = scene.placement();
    let transformed = !place.is_identity();
    if transformed {
        snapshot.save();
        #[allow(clippy::cast_possible_truncation)]
        snapshot.translate(&graphene::Point::new(place.x as f32, place.y as f32));
        #[allow(clippy::cast_possible_truncation)]
        snapshot.scale(place.scale as f32, place.scale as f32);
    }

    if let Some(texture) = base {
        append_base(snapshot, texture, &scene.base);
    }

    // Then everything else, in the order the scene decided.
    for object in &order {
        if matches!(object.geometry, Geometry::Background { .. }) {
            continue;
        }
        draw(snapshot, object);
    }

    if transformed {
        snapshot.restore();
    }
    for _ in 0..pushed {
        snapshot.pop();
    }
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
/// the document has no background and the picture is simply the canvas.
fn picture_frame(scene: &Scene) -> Option<(Bounds, f64, Option<gsk::Shadow>)> {
    let (source, params) = scene.background_source()?;
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
                let placed =
                    cover(canvas, f64::from(texture.width()), f64::from(texture.height()));
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
                let placed =
                    cover(canvas, f64::from(texture.width()), f64::from(texture.height()));
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
    let start = graphene::Point::new(
        (centre.x - dx * reach) as f32,
        (centre.y - dy * reach) as f32,
    );
    let end = graphene::Point::new(
        (centre.x + dx * reach) as f32,
        (centre.y + dy * reach) as f32,
    );
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
        snapshot.append_radial_gradient(&rect, &at, radius, radius, 0.0, 1.0, &[
            gsk::ColorStop::new(0.0, colour),
            gsk::ColorStop::new(1.0, clear),
        ]);
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
    let shadowed = object.style.shadow && !matches!(
        object.geometry,
        // A redaction with a shadow would draw a soft edge around the thing it is hiding,
        // which points at it. A spotlight's whole job is the mask; a shadow inside it is
        // a second edge nobody asked for.
        Geometry::Redact { .. } | Geometry::Spotlight { .. } | Geometry::Background { .. }
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
        Geometry::Path { points, smoothing, highlighter, band } => {
            append_freehand(snapshot, points, *smoothing, *highlighter, *band, width, &color);
        }
        Geometry::Counter { center, number, style, radius } => {
            append_counter(snapshot, *center, *number, *style, *radius, &color);
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
    let Some(first_object) = order
        .iter()
        .find(|o| matches!(o.geometry, Geometry::Spotlight { .. }))
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
            SpotlightShape::Rounded => rounded_rect_path(*bounds, 12.0),
            SpotlightShape::Rectangle => rect_path(*bounds),
        };
        snapshot.append_fill(&path, gsk::FillRule::Winding, &gdk::RGBA::BLACK);
    }
    snapshot.pop();

    #[allow(clippy::cast_possible_truncation)]
    let scrim = gdk::RGBA::new(0.0, 0.0, 0.0, opacity as f32);
    // Over the whole canvas, not over the union: the point is that everything *else* is
    // darkened, and a scrim the size of the shapes would darken nothing.
    snapshot.append_color(&scrim, &to_rect(scene.canvas.union(union)));
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
        builder.line_to(
            (head_end.x + perp.0 * half) as f32,
            (head_end.y + perp.1 * half) as f32,
        );
        builder.line_to(
            (head_end.x - perp.0 * half) as f32,
            (head_end.y - perp.1 * half) as f32,
        );
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

/// `spec/05` §4.9's numbered badge. Always on top, which the scene's order already did.
fn append_counter(
    snapshot: &gtk::Snapshot,
    center: Point,
    number: u32,
    style: octosnap_scene::CounterStyle,
    radius: f64,
    color: &gdk::RGBA,
) {
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
        builder.quad_to(
            window[1].x as f32,
            window[1].y as f32,
            mid.x as f32,
            mid.y as f32,
        );
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
