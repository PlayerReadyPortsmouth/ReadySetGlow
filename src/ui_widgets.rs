//! Custom egui widgets: glowing LED strips, device previews, the gradient
//! palette editor, the device layout editor and the spectrum meter.

use crate::audio::AudioFeatures;
use crate::color::Rgb;
use crate::devices::{DeviceConfig, OutputMode};
use crate::palette::{self, Palette, Stop};
use eframe::egui::{
    self, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};

pub const BG: Color32 = Color32::from_rgb(12, 13, 18);
pub const PANEL: Color32 = Color32::from_rgb(20, 22, 30);
pub const RAISED: Color32 = Color32::from_rgb(30, 33, 44);
pub const LINE: Color32 = Color32::from_rgb(48, 52, 68);
pub const ACCENT: Color32 = Color32::from_rgb(255, 79, 163);
pub const ACCENT2: Color32 = Color32::from_rgb(79, 240, 255);
pub const DIM: Color32 = Color32::from_rgb(140, 146, 168);
pub const LIVE: Color32 = Color32::from_rgb(64, 230, 120);

pub fn c32(c: Rgb) -> Color32 {
    let [r, g, b] = c.to_u8();
    Color32::from_rgb(r, g, b)
}

pub fn c32u(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn glow(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// A glowing strip of LEDs filling the available width.
pub fn led_strip(ui: &mut egui::Ui, pixels: &[Color32], height: f32) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let p = ui.painter_at(rect.expand(height));
    p.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(6, 7, 10));
    let n = pixels.len().max(1);
    let inner = rect.shrink2(vec2(8.0, height * 0.25));
    let cell = inner.width() / n as f32;
    let led = (cell * 0.7).clamp(1.5, inner.height());
    for (i, &c) in pixels.iter().enumerate() {
        let cx = inner.left() + cell * (i as f32 + 0.5);
        let centre = pos2(cx, inner.center().y);
        let lum = c.r().max(c.g()).max(c.b());
        if lum > 8 {
            // Soft bloom: a couple of translucent discs behind each LED.
            p.circle_filled(centre, led * 2.6, glow(c, (lum as f32 * 0.10) as u8));
            p.circle_filled(centre, led * 1.4, glow(c, (lum as f32 * 0.25) as u8));
        }
        let r = Rect::from_center_size(centre, vec2(led.max(cell * 0.6), inner.height() * 0.55));
        p.rect_filled(
            r,
            CornerRadius::same(2),
            if lum > 4 {
                c
            } else {
                Color32::from_rgb(22, 24, 30)
            },
        );
    }
    resp
}

/// What a device is (or would be) showing, drawn to match its capability.
pub fn device_preview(
    ui: &mut egui::Ui,
    mode: &OutputMode,
    shown: &[[u8; 3]],
    size: Vec2,
    enabled: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter_at(rect.expand(16.0));
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.25) };
    match mode {
        OutputMode::Segments { .. } | OutputMode::Stream { .. } => {
            p.rect_filled(rect, CornerRadius::same(4), Color32::from_rgb(6, 7, 10));
            let n = shown.len().max(1);
            let w = rect.width() / n as f32;
            for (i, &c) in shown.iter().enumerate() {
                let col = fade(c32u(c));
                let r = Rect::from_min_size(
                    pos2(rect.left() + w * i as f32, rect.top()),
                    vec2(w, rect.height()),
                );
                p.rect_filled(r.expand(3.0), CornerRadius::same(4), glow(col, 30));
                p.rect_filled(
                    r.shrink(if w > 4.0 { 1.0 } else { 0.0 }),
                    CornerRadius::same(2),
                    col,
                );
            }
        }
        _ => {
            let c = fade(shown.first().map(|&c| c32u(c)).unwrap_or(Color32::BLACK));
            let centre = rect.center();
            let r = rect.height().min(rect.width()) * 0.5;
            let lum = c.r().max(c.g()).max(c.b()) as f32;
            p.circle_filled(centre, r * 1.5, glow(c, (lum * 0.12) as u8));
            p.circle_filled(centre, r * 1.15, glow(c, (lum * 0.25) as u8));
            p.circle_filled(centre, r * 0.8, c);
            p.circle_stroke(centre, r * 0.8, Stroke::new(1.0_f32, LINE));
            if matches!(mode, OutputMode::OnOff) {
                p.text(
                    centre,
                    egui::Align2::CENTER_CENTER,
                    "🔌",
                    egui::FontId::proportional(r * 0.8),
                    Color32::from_black_alpha(160),
                );
            }
        }
    }
    resp
}

/// Gradient editor. Click the bar to add a stop, drag stops to move them,
/// right-click a stop to remove it. Returns true if anything changed.
pub fn palette_editor(
    ui: &mut egui::Ui,
    pal: &mut Palette,
    selected: &mut usize,
    id: egui::Id,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt(id.with("builtin"))
            .selected_text("Load palette…")
            .width(130.0)
            .show_ui(ui, |ui| {
                for name in palette::BUILTIN_NAMES {
                    let preview = palette::builtin(name).unwrap();
                    ui.horizontal(|ui| {
                        gradient_swatch(ui, &preview, vec2(60.0, 12.0));
                        if ui.selectable_label(false, *name).clicked() {
                            *pal = preview.clone();
                            *selected = 0;
                            changed = true;
                        }
                    });
                }
            });
        if ui
            .checkbox(&mut pal.wrap, "Loop")
            .on_hover_text("Blend the last colour back into the first")
            .changed()
        {
            changed = true;
        }
        if ui.small_button("⇄").on_hover_text("Reverse").clicked() {
            for s in &mut pal.stops {
                s.pos = 1.0 - s.pos;
            }
            pal.sort();
            changed = true;
        }
        if ui
            .small_button("🎲")
            .on_hover_text("Random palette")
            .clicked()
        {
            let base: f32 = (ui.input(|i| i.time) * 0.618).fract() as f32;
            let n = 3 + (ui.input(|i| i.time) as usize % 3);
            pal.stops = (0..n)
                .map(|i| Stop {
                    pos: i as f32 / n as f32,
                    color: Rgb::hsv(base + i as f32 * 0.17 + (i % 2) as f32 * 0.31, 0.85, 1.0),
                })
                .collect();
            pal.wrap = true;
            *selected = 0;
            changed = true;
        }
    });

    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 46.0), Sense::click_and_drag());
    let bar = Rect::from_min_size(rect.min, vec2(w, 26.0));
    let p = ui.painter_at(rect);
    paint_gradient(&p, bar, pal);
    p.rect_stroke(
        bar,
        CornerRadius::same(4),
        Stroke::new(1.0_f32, LINE),
        StrokeKind::Inside,
    );

    let handle_y = bar.bottom() + 9.0;
    let to_x = |pos: f32| bar.left() + pos.clamp(0.0, 1.0) * bar.width();
    let from_x = |x: f32| ((x - bar.left()) / bar.width()).clamp(0.0, 1.0);

    // Hit test handles first.
    // On drag start, hit-test where the button went down (not where the drag threshold was crossed).
    let origin = if resp.drag_started() {
        ui.input(|i| i.pointer.press_origin())
    } else {
        None
    };
    let pointer = origin.or(resp.interact_pointer_pos()).or(resp.hover_pos());
    let hit = pointer.and_then(|pp| {
        pal.stops
            .iter()
            .enumerate()
            .map(|(i, s)| (i, (pos2(to_x(s.pos), handle_y) - pp).length()))
            .filter(|(_, d)| *d < 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    });

    let drag_id = id.with("drag");
    if resp.drag_started()
        && let Some(i) = hit
    {
        *selected = i;
        ui.data_mut(|d| d.insert_temp(drag_id, i));
    }
    if resp.dragged()
        && let (Some(i), Some(pp)) = (
            ui.data(|d| d.get_temp::<usize>(drag_id)),
            resp.interact_pointer_pos(),
        )
        && let Some(s) = pal.stops.get_mut(i)
    {
        s.pos = from_x(pp.x);
        changed = true;
    }
    if resp.drag_stopped() {
        // Keep the selection on the same stop after sorting.
        if let Some(i) = ui.data(|d| d.get_temp::<usize>(drag_id))
            && let Some(stop) = pal.stops.get(i).cloned()
        {
            pal.sort();
            *selected = pal.stops.iter().position(|s| *s == stop).unwrap_or(0);
        }
        ui.data_mut(|d| d.remove::<usize>(drag_id));
    }
    if resp.clicked()
        && let Some(pp) = resp.interact_pointer_pos()
    {
        match hit {
            Some(i) => *selected = i,
            None if bar.contains(pp) => {
                let pos = from_x(pp.x);
                let color = pal.sample(pos);
                pal.stops.push(Stop { pos, color });
                pal.sort();
                *selected = pal.stops.iter().position(|s| s.pos == pos).unwrap_or(0);
                changed = true;
            }
            None => {}
        }
    }
    if resp.secondary_clicked()
        && pal.stops.len() > 2
        && let Some(i) = hit
    {
        pal.stops.remove(i);
        *selected = 0;
        changed = true;
    }

    for (i, s) in pal.stops.iter().enumerate() {
        let x = to_x(s.pos);
        let sel = i == *selected;
        let tip = pos2(x, bar.bottom() + 1.0);
        let pts = vec![
            tip,
            pos2(x - 7.0, handle_y + 6.0),
            pos2(x + 7.0, handle_y + 6.0),
        ];
        p.add(egui::Shape::convex_polygon(
            pts,
            c32(s.color),
            Stroke::new(
                if sel { 2.0_f32 } else { 1.0_f32 },
                if sel { Color32::WHITE } else { LINE },
            ),
        ));
    }
    if hit.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    if *selected >= pal.stops.len() {
        *selected = 0;
    }
    if let Some(s) = pal.stops.get_mut(*selected) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Stop").color(DIM));
            let mut c = s.color.to_u8();
            if egui::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
                s.color = Rgb::from_u8(c[0], c[1], c[2]);
                changed = true;
            }
            if ui
                .add(
                    egui::DragValue::new(&mut s.pos)
                        .range(0.0..=1.0)
                        .speed(0.005)
                        .fixed_decimals(2),
                )
                .changed()
            {
                changed = true;
            }
            ui.label(
                egui::RichText::new("click bar: add · right-click stop: remove")
                    .small()
                    .color(DIM),
            );
        });
    }
    changed
}

pub fn paint_gradient(p: &egui::Painter, rect: Rect, pal: &Palette) {
    let steps = (rect.width() / 3.0).max(8.0) as usize;
    let w = rect.width() / steps as f32;
    for i in 0..steps {
        let t = (i as f32 + 0.5) / steps as f32;
        let r = Rect::from_min_size(
            pos2(rect.left() + w * i as f32, rect.top()),
            vec2(w + 0.6, rect.height()),
        );
        p.rect_filled(r, CornerRadius::ZERO, c32(pal.sample(t)));
    }
}

pub fn gradient_swatch(ui: &mut egui::Ui, pal: &Palette, size: Vec2) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint_gradient(ui.painter(), rect, pal);
    resp
}

/// Drag devices along the canvas. Each row is one device; drag the bar to
/// move it, drag its ends to resize. Returns true if a span changed.
pub fn layout_editor(
    ui: &mut egui::Ui,
    canvas: &[Color32],
    devices: &mut [DeviceConfig],
    shown: &dyn Fn(&str) -> Vec<[u8; 3]>,
    selected: &mut Option<String>,
) -> bool {
    let mut changed = false;
    let w = ui.available_width();
    let row_h = 30.0;
    let label_w = 150.0;
    let track_w = (w - label_w - 8.0).max(100.0);

    // Canvas reference strip aligned with the tracks.
    ui.horizontal(|ui| {
        ui.add_sized(
            [label_w, 18.0],
            egui::Label::new(egui::RichText::new("Canvas").color(DIM)),
        );
        let (rect, _) = ui.allocate_exact_size(vec2(track_w, 18.0), Sense::hover());
        let p = ui.painter_at(rect);
        let n = canvas.len().max(1);
        let cw = rect.width() / n as f32;
        for (i, &c) in canvas.iter().enumerate() {
            p.rect_filled(
                Rect::from_min_size(
                    pos2(rect.left() + cw * i as f32, rect.top()),
                    vec2(cw + 0.5, 18.0),
                ),
                CornerRadius::ZERO,
                c,
            );
        }
    });

    for d in devices.iter_mut() {
        ui.horizontal(|ui| {
            let is_sel = selected.as_deref() == Some(d.id.as_str());
            let name = egui::RichText::new(d.display_name()).color(if d.enabled {
                Color32::WHITE
            } else {
                DIM
            });
            if ui
                .add_sized([label_w, row_h], egui::Button::selectable(is_sel, name))
                .clicked()
            {
                *selected = Some(d.id.clone());
            }
            let (track, resp) =
                ui.allocate_exact_size(vec2(track_w, row_h), Sense::click_and_drag());
            let p = ui.painter_at(track);
            p.rect_filled(track, CornerRadius::same(6), PANEL);
            for k in 1..10 {
                let x = track.left() + track.width() * k as f32 / 10.0;
                p.line_segment(
                    [pos2(x, track.top() + 4.0), pos2(x, track.bottom() - 4.0)],
                    Stroke::new(1.0_f32, RAISED),
                );
            }
            let (a, b) = (d.span_start.min(d.span_end), d.span_start.max(d.span_end));
            let bar = Rect::from_x_y_ranges(
                track.left() + a * track.width()..=track.left() + b * track.width(),
                track.top() + 3.0..=track.bottom() - 3.0,
            );

            // Fill the bar with what the device shows, so you can see the mapping.
            let px = shown(&d.id);
            let n = px.len().max(1);
            let seg = bar.width() / n as f32;
            for (i, &c) in px.iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(bar.left() + seg * i as f32, bar.top()),
                    vec2(seg + 0.5, bar.height()),
                );
                p.rect_filled(
                    r,
                    CornerRadius::ZERO,
                    if d.enabled {
                        c32u(c)
                    } else {
                        c32u(c).gamma_multiply(0.3)
                    },
                );
            }
            p.rect_stroke(
                bar,
                CornerRadius::same(4),
                Stroke::new(
                    if is_sel { 2.0_f32 } else { 1.0_f32 },
                    if is_sel {
                        ACCENT
                    } else {
                        Color32::from_white_alpha(90)
                    },
                ),
                StrokeKind::Inside,
            );
            if d.reverse {
                p.text(
                    bar.right_center() - vec2(10.0, 0.0),
                    egui::Align2::CENTER_CENTER,
                    "◀",
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }

            let key = ui.id().with(("span", &d.id));
            let to_frac = |x: f32| ((x - track.left()) / track.width()).clamp(0.0, 1.0);
            if let Some(pp) = resp.hover_pos() {
                let near_edge = (pp.x - bar.left()).abs() < 7.0 || (pp.x - bar.right()).abs() < 7.0;
                ui.ctx().set_cursor_icon(if near_edge {
                    egui::CursorIcon::ResizeHorizontal
                } else if bar.contains(pp) {
                    egui::CursorIcon::Grab
                } else {
                    egui::CursorIcon::Default
                });
            }
            if resp.drag_started() {
                *selected = Some(d.id.clone());
                let origin = ui
                    .input(|i| i.pointer.press_origin())
                    .or(resp.interact_pointer_pos());
                if let Some(pp) = origin {
                    let mode: u8 = if (pp.x - bar.left()).abs() < 7.0 {
                        1
                    } else if (pp.x - bar.right()).abs() < 7.0 {
                        2
                    } else {
                        0
                    };
                    ui.data_mut(|m| m.insert_temp(key, (mode, to_frac(pp.x), a, b)));
                }
            }
            if resp.dragged()
                && let (Some((mode, grab, a0, b0)), Some(pp)) = (
                    ui.data(|m| m.get_temp::<(u8, f32, f32, f32)>(key)),
                    resp.interact_pointer_pos(),
                )
            {
                let x = to_frac(pp.x);
                let snap = |v: f32| {
                    if ui.input(|i| i.modifiers.shift) {
                        v
                    } else {
                        (v * 40.0).round() / 40.0
                    }
                };
                match mode {
                    1 => d.span_start = snap(x).min(b0 - 0.01),
                    2 => d.span_end = snap(x).max(a0 + 0.01),
                    _ => {
                        let width = b0 - a0;
                        let na = snap((a0 + x - grab).clamp(0.0, 1.0 - width));
                        d.span_start = na;
                        d.span_end = (na + width).min(1.0);
                    }
                }
                changed = true;
            }
            if resp.clicked() {
                *selected = Some(d.id.clone());
            }
            if resp.double_clicked() {
                d.reverse = !d.reverse;
                changed = true;
            }
        });
    }
    ui.label(
        egui::RichText::new(
            "Drag bars to move · drag ends to resize (Shift = no snap) · double-click to reverse",
        )
        .small()
        .color(DIM),
    );
    changed
}

pub fn spectrum(ui: &mut egui::Ui, a: &AudioFeatures, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(6, 7, 10));
    let n = a.spectrum.len();
    let w = rect.width() / n as f32;
    for (i, &v) in a.spectrum.iter().enumerate() {
        let h = v.clamp(0.0, 1.0) * rect.height();
        let t = i as f32 / n as f32;
        let col = c32(Rgb::hsv(0.85 - t * 0.35, 0.8, 1.0));
        p.rect_filled(
            Rect::from_min_max(
                pos2(rect.left() + w * i as f32 + 1.0, rect.bottom() - h),
                pos2(rect.left() + w * (i + 1) as f32 - 1.0, rect.bottom()),
            ),
            CornerRadius::same(1),
            col,
        );
    }
    if a.beat_pulse > 0.05 {
        p.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(2.0_f32, glow(ACCENT, (a.beat_pulse * 255.0) as u8)),
            StrokeKind::Inside,
        );
    }
}

/// Little beat indicator: four dots, the current one lit.
pub fn beat_dots(ui: &mut egui::Ui, beats: f64) {
    let (rect, _) = ui.allocate_exact_size(vec2(44.0, 14.0), Sense::hover());
    let p = ui.painter_at(rect);
    let cur = (beats.floor() as i64).rem_euclid(4) as usize;
    let frac = beats.fract() as f32;
    for i in 0..4 {
        let c = pos2(rect.left() + 6.0 + i as f32 * 11.0, rect.center().y);
        let lit = i == cur;
        let col = if lit {
            glow(
                if i == 0 { ACCENT } else { ACCENT2 },
                (255.0 * (1.0 - frac * 0.7)) as u8,
            )
        } else {
            RAISED
        };
        p.circle_filled(c, 4.0, col);
    }
}

pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .small()
            .strong()
            .color(DIM),
    );
    ui.add_space(2.0);
}

pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = RAISED;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color = ACCENT2;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    v.widgets.inactive.bg_fill = RAISED;
    v.widgets.inactive.weak_bg_fill = RAISED;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(44, 48, 64);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(58, 62, 82);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(6);
    }
    v.window_corner_radius = CornerRadius::same(10);
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    // Fall back to Hack for symbols (arrows, ●, ▲...) the UI font lacks.
    let mut fonts = egui::FontDefinitions::default();
    if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        fam.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(8.0, 4.0);
        s.spacing.slider_width = 170.0;
    });
}
