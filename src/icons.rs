use eframe::egui::{
    self, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};

use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Prev,
    Next,
    Shuffle,
    Repeat,
    RepeatOne,
    HeartOutline,
    HeartFilled,
    VolumeHigh,
    VolumeLow,
    VolumeMuted,
    Library,
    Queue,
    Playlist,
    Folder,
    FolderPlus,
    TrackPlus,
    PlayNext,
    Plus,
    Trash,
    Info,
    Tag,
    Settings,
    External,
    Search,
    Close,
    Refresh,
    ArrowUp,
    ArrowDown,
    Device,
    Visualizer,
    Palette,
    Check,
    Plugin,
    Sliders,
    Compact,
    Expand,
    List,
    Lyrics,
    StarFilled,
    Grid,
}

#[inline]
pub fn snap_px(v: f32, ppp: f32) -> f32 {
    (v * ppp).round() / ppp
}

#[inline]
pub fn snap_pos(p: Pos2, ppp: f32) -> Pos2 {
    pos2(snap_px(p.x, ppp), snap_px(p.y, ppp))
}

#[inline]
pub fn snap_rect(r: Rect, ppp: f32) -> Rect {
    Rect::from_min_max(snap_pos(r.min, ppp), snap_pos(r.max, ppp))
}

pub fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    if color.a() == 0 {
        return;
    }
    let ppp = painter.pixels_per_point().max(1.0);
    let c = snap_pos(rect.center(), ppp);
    let s = rect.width().min(rect.height());
    let stroke_w = ((s * 0.115).clamp(1.25, 2.0) * ppp).round().max(1.0) / ppp;
    let stroke = Stroke::new(stroke_w, color);

    match icon {
        Icon::Play => {
            let r = s * 0.36;
            let pts = vec![
                pos2(c.x - r * 0.65, c.y - r),
                pos2(c.x + r * 0.95, c.y),
                pos2(c.x - r * 0.65, c.y + r),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Pause => {
            let w = snap_px((s * 0.22).max(2.5), ppp);
            let h = snap_px(s * 0.64, ppp);
            let gap = snap_px(s * 0.14, ppp);
            let left = snap_rect(
                Rect::from_center_size(pos2(c.x - gap - w * 0.5, c.y), vec2(w, h)),
                ppp,
            );
            let right = snap_rect(
                Rect::from_center_size(pos2(c.x + gap + w * 0.5, c.y), vec2(w, h)),
                ppp,
            );
            painter.rect_filled(left, 1.2, color);
            painter.rect_filled(right, 1.2, color);
        }
        Icon::Prev => {
            let r = s * 0.32;
            let bar = snap_rect(
                Rect::from_center_size(pos2(c.x - r * 0.85, c.y), vec2(2.0, r * 1.9)),
                ppp,
            );
            painter.rect_filled(bar, 0.8, color);
            let pts = vec![
                pos2(c.x + r * 0.9, c.y - r * 0.95),
                pos2(c.x - r * 0.55, c.y),
                pos2(c.x + r * 0.9, c.y + r * 0.95),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Next => {
            let r = s * 0.32;
            let bar = snap_rect(
                Rect::from_center_size(pos2(c.x + r * 0.85, c.y), vec2(2.0, r * 1.9)),
                ppp,
            );
            painter.rect_filled(bar, 0.8, color);
            let pts = vec![
                pos2(c.x - r * 0.9, c.y - r * 0.95),
                pos2(c.x + r * 0.55, c.y),
                pos2(c.x - r * 0.9, c.y + r * 0.95),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Shuffle => {
            let r = s * 0.36;
            painter.line_segment(
                [pos2(c.x - r, c.y - r * 0.55), pos2(c.x - r * 0.2, c.y - r * 0.55)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x - r * 0.2, c.y - r * 0.55), pos2(c.x + r * 0.35, c.y + r * 0.55)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x + r * 0.35, c.y + r * 0.55), pos2(c.x + r, c.y + r * 0.55)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x - r, c.y + r * 0.55), pos2(c.x - r * 0.2, c.y + r * 0.55)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x - r * 0.2, c.y + r * 0.55), pos2(c.x + r * 0.35, c.y - r * 0.55)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x + r * 0.35, c.y - r * 0.55), pos2(c.x + r, c.y - r * 0.55)],
                stroke,
            );
            let ah = r * 0.32;
            for sy in [-0.55_f32, 0.55] {
                painter.line_segment(
                    [pos2(c.x + r - ah, c.y + r * sy - ah * 0.7), pos2(c.x + r, c.y + r * sy)],
                    stroke,
                );
                painter.line_segment(
                    [pos2(c.x + r - ah, c.y + r * sy + ah * 0.7), pos2(c.x + r, c.y + r * sy)],
                    stroke,
                );
            }
        }
        Icon::Repeat | Icon::RepeatOne => {
            let r = s * 0.36;
            let loop_rect = snap_rect(Rect::from_center_size(c, vec2(r * 2.0, r * 1.25)), ppp);
            painter.rect_stroke(loop_rect, r * 0.35, stroke, StrokeKind::Middle);
            let tip = pos2(loop_rect.right() - r * 0.25, loop_rect.top());
            painter.line_segment([pos2(tip.x - r * 0.28, tip.y - r * 0.25), tip], stroke);
            painter.line_segment([pos2(tip.x - r * 0.28, tip.y + r * 0.25), tip], stroke);
            if icon == Icon::RepeatOne {
                painter.text(
                    c,
                    Align2::CENTER_CENTER,
                    "1",
                    FontId::proportional((s * 0.44).max(8.0)),
                    color,
                );
            }
        }
        // Cusp-free geometric heart: ZERO miter spikes at cleft or tip at any DPI!
        Icon::HeartFilled => {
            let r = s * 0.36;
            let lobe_r = r * 0.52;
            let lobe_y = c.y - r * 0.22;
            let lobe_dx = r * 0.45;
            let left_c = pos2(c.x - lobe_dx, lobe_y);
            let right_c = pos2(c.x + lobe_dx, lobe_y);
            let tip = pos2(c.x, c.y + r * 0.96);

            painter.circle_filled(left_c, lobe_r, color);
            painter.circle_filled(right_c, lobe_r, color);
            // Smooth convex body connecting outer flanks of both lobes to the bottom tip
            let flank_y = lobe_y + lobe_r * 0.36;
            let flank_x = lobe_dx + lobe_r * 0.93;
            let body = vec![
                pos2(c.x - flank_x, flank_y),
                pos2(c.x, lobe_y - lobe_r * 0.15),
                pos2(c.x + flank_x, flank_y),
                tip,
            ];
            painter.add(Shape::convex_polygon(body, color, Stroke::NONE));
        }
        Icon::HeartOutline => {
            // Draw heart outline as two smooth open polylines (left half + right half) meeting cleanly
            // at the top cleft and bottom tip — completely eliminates closed_line 180° miter spikes!
            let r = s * 0.36;
            let lobe_r = r * 0.50;
            let lobe_y = c.y - r * 0.20;
            let lobe_dx = r * 0.44;
            let cleft = pos2(c.x, lobe_y + lobe_r * 0.08);
            let tip = pos2(c.x, c.y + r * 0.92);

            let build_half = |sign: f32| -> Vec<Pos2> {
                let lc = pos2(c.x + sign * lobe_dx, lobe_y);
                let mut pts = Vec::with_capacity(14);
                pts.push(cleft);
                // Arc around lobe from inner cleft over top to outer lower flank
                let steps = 9;
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    // For right (+1): angle from PI -> 0 -> 0.28*PI (clockwise on screen y-down)
                    let a = if sign > 0.0 {
                        std::f32::consts::PI - t * (std::f32::consts::PI * 1.24)
                    } else {
                        t * (std::f32::consts::PI * 1.24)
                    };
                    let px = lc.x + lobe_r * a.cos();
                    let py = lc.y - lobe_r * a.sin();
                    pts.push(pos2(px, py));
                }
                // Smooth quadratic curve down to tip
                let p0 = *pts.last().unwrap();
                let ctrl = pos2(c.x + sign * (lobe_dx + lobe_r * 0.42), c.y + r * 0.48);
                for i in 1..=4 {
                    let t = i as f32 / 4.0;
                    let mt = 1.0 - t;
                    let px = mt * mt * p0.x + 2.0 * mt * t * ctrl.x + t * t * tip.x;
                    let py = mt * mt * p0.y + 2.0 * mt * t * ctrl.y + t * t * tip.y;
                    pts.push(pos2(px, py));
                }
                pts
            };

            let left_pts = build_half(-1.0);
            let right_pts = build_half(1.0);
            painter.add(Shape::line(left_pts, stroke));
            painter.add(Shape::line(right_pts, stroke));
        }
        Icon::VolumeHigh | Icon::VolumeLow | Icon::VolumeMuted => {
            let r = s * 0.34;
            let speaker = vec![
                pos2(c.x - r * 0.95, c.y - r * 0.32),
                pos2(c.x - r * 0.45, c.y - r * 0.32),
                pos2(c.x + r * 0.05, c.y - r * 0.8),
                pos2(c.x + r * 0.05, c.y + r * 0.8),
                pos2(c.x - r * 0.45, c.y + r * 0.32),
                pos2(c.x - r * 0.95, c.y + r * 0.32),
            ];
            painter.add(Shape::convex_polygon(speaker, color, Stroke::NONE));
            match icon {
                Icon::VolumeMuted => {
                    let x0 = c.x + r * 0.32;
                    let x1 = c.x + r * 0.95;
                    let dy = r * 0.35;
                    painter.line_segment([pos2(x0, c.y - dy), pos2(x1, c.y + dy)], stroke);
                    painter.line_segment([pos2(x0, c.y + dy), pos2(x1, c.y - dy)], stroke);
                }
                Icon::VolumeLow => {
                    paint_arc(painter, pos2(c.x + r * 0.15, c.y), r * 0.45, -0.75, 0.75, stroke);
                }
                Icon::VolumeHigh => {
                    paint_arc(painter, pos2(c.x + r * 0.12, c.y), r * 0.42, -0.75, 0.75, stroke);
                    paint_arc(painter, pos2(c.x + r * 0.12, c.y), r * 0.80, -0.82, 0.82, stroke);
                }
                _ => {}
            }
        }
        Icon::Library => {
            let r = s * 0.36;
            let bw = snap_px((s * 0.15).max(1.8), ppp);
            let gap = snap_px(s * 0.13, ppp);
            let total_w = bw * 3.0 + gap * 2.0;
            let start_x = snap_px(c.x - total_w * 0.5, ppp);
            let bottom = snap_px(c.y + r * 0.85, ppp);
            for (i, h) in [0.76_f32, 0.96, 0.82].iter().enumerate() {
                let x = snap_px(start_x + i as f32 * (bw + gap), ppp);
                let bh = snap_px(r * 1.85 * h, ppp);
                painter.rect_filled(
                    Rect::from_min_max(pos2(x, bottom - bh), pos2(x + bw, bottom)),
                    0.8,
                    color,
                );
            }
        }
        Icon::Queue | Icon::List => {
            let r = s * 0.35;
            for offset in [-0.55_f32, 0.0, 0.55] {
                let y = snap_px(c.y + r * offset, ppp);
                painter.line_segment(
                    [pos2(snap_px(c.x - r * 0.88, ppp), y), pos2(snap_px(c.x + r * 0.88, ppp), y)],
                    stroke,
                );
            }
        }
        Icon::Playlist => {
            let r = s * 0.36;
            for (dy, right_f) in [(-0.55_f32, 0.35_f32), (0.0, 0.15), (0.55, 0.15)] {
                let y = snap_px(c.y + r * dy, ppp);
                painter.line_segment([pos2(c.x - r * 0.9, y), pos2(c.x + r * right_f, y)], stroke);
            }
            let head = pos2(c.x + r * 0.55, c.y + r * 0.45);
            painter.circle_filled(head, r * 0.24, color);
            painter.line_segment(
                [pos2(head.x + r * 0.18, head.y), pos2(head.x + r * 0.18, c.y - r * 0.55)],
                stroke,
            );
        }
        Icon::Folder | Icon::FolderPlus => {
            let r = s * 0.36;
            let body = snap_rect(
                Rect::from_min_max(pos2(c.x - r, c.y - r * 0.42), pos2(c.x + r, c.y + r * 0.8)),
                ppp,
            );
            painter.rect_stroke(body, 1.8, stroke, StrokeKind::Middle);
            let tab_pts = vec![
                pos2(body.left(), body.top()),
                pos2(body.left(), snap_px(c.y - r * 0.80, ppp)),
                pos2(snap_px(c.x - r * 0.18, ppp), snap_px(c.y - r * 0.80, ppp)),
                pos2(snap_px(c.x + r * 0.05, ppp), body.top()),
            ];
            painter.add(Shape::line(tab_pts, stroke));
            if icon == Icon::FolderPlus {
                let pr = r * 0.34;
                let pc = snap_pos(pos2(c.x, c.y + r * 0.19), ppp);
                painter.line_segment([pos2(pc.x - pr, pc.y), pos2(pc.x + pr, pc.y)], stroke);
                painter.line_segment([pos2(pc.x, pc.y - pr), pos2(pc.x, pc.y + pr)], stroke);
            }
        }
        Icon::TrackPlus => {
            let r = s * 0.36;
            let head = pos2(c.x - r * 0.38, c.y + r * 0.48);
            painter.circle_filled(head, r * 0.26, color);
            painter.line_segment(
                [pos2(head.x + r * 0.2, head.y), pos2(head.x + r * 0.2, c.y - r * 0.72)],
                stroke,
            );
            painter.line_segment(
                [pos2(head.x + r * 0.2, c.y - r * 0.72), pos2(c.x + r * 0.45, c.y - r * 0.52)],
                stroke,
            );
            let pc = pos2(c.x + r * 0.58, c.y + r * 0.35);
            let pr = r * 0.34;
            painter.line_segment([pos2(pc.x - pr, pc.y), pos2(pc.x + pr, pc.y)], stroke);
            painter.line_segment([pos2(pc.x, pc.y - pr), pos2(pc.x, pc.y + pr)], stroke);
        }
        Icon::PlayNext => {
            let r = s * 0.35;
            let pts = vec![
                pos2(c.x - r * 0.9, c.y - r * 0.75),
                pos2(c.x - r * 0.05, c.y - r * 0.2),
                pos2(c.x - r * 0.9, c.y + r * 0.35),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
            painter.line_segment(
                [pos2(c.x + r * 0.15, c.y - r * 0.5), pos2(c.x + r * 0.95, c.y - r * 0.5)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x + r * 0.15, c.y + r * 0.05), pos2(c.x + r * 0.95, c.y + r * 0.05)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x - r * 0.9, c.y + r * 0.62), pos2(c.x + r * 0.95, c.y + r * 0.62)],
                stroke,
            );
        }
        Icon::Plus => {
            let r = s * 0.32;
            painter.line_segment([pos2(c.x - r, c.y), pos2(c.x + r, c.y)], stroke);
            painter.line_segment([pos2(c.x, c.y - r), pos2(c.x, c.y + r)], stroke);
        }
        Icon::Trash => {
            let r = s * 0.36;
            painter.line_segment(
                [pos2(c.x - r * 0.85, c.y - r * 0.55), pos2(c.x + r * 0.85, c.y - r * 0.55)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x - r * 0.35, c.y - r * 0.85), pos2(c.x + r * 0.35, c.y - r * 0.85)],
                stroke,
            );
            let body = snap_rect(
                Rect::from_min_max(
                    pos2(c.x - r * 0.62, c.y - r * 0.55),
                    pos2(c.x + r * 0.62, c.y + r * 0.85),
                ),
                ppp,
            );
            painter.rect_stroke(body, 1.5, stroke, StrokeKind::Middle);
            painter.line_segment(
                [pos2(c.x - r * 0.2, c.y - r * 0.2), pos2(c.x - r * 0.2, c.y + r * 0.5)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x + r * 0.2, c.y - r * 0.2), pos2(c.x + r * 0.2, c.y + r * 0.5)],
                stroke,
            );
        }
        Icon::Info => {
            let r = s * 0.37;
            painter.circle_stroke(c, r, stroke);
            painter.circle_filled(pos2(c.x, c.y - r * 0.42), (s * 0.075).max(1.1), color);
            painter.line_segment([pos2(c.x, c.y - r * 0.1), pos2(c.x, c.y + r * 0.48)], stroke);
        }
        Icon::Tag => {
            let r = s * 0.36;
            let pts = vec![
                pos2(c.x - r * 0.82, c.y - r * 0.82),
                pos2(c.x + r * 0.05, c.y - r * 0.82),
                pos2(c.x + r * 0.86, c.y + r * 0.02),
                pos2(c.x + r * 0.02, c.y + r * 0.86),
                pos2(c.x - r * 0.82, c.y + r * 0.05),
            ];
            painter.add(Shape::closed_line(pts, stroke));
            painter.circle_filled(pos2(c.x - r * 0.38, c.y - r * 0.38), (s * 0.08).max(1.2), color);
        }
        Icon::Settings => {
            let r = s * 0.35;
            painter.circle_stroke(c, r * 0.48, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                painter.line_segment(
                    [
                        pos2(c.x + r * 0.65 * a.cos(), c.y + r * 0.65 * a.sin()),
                        pos2(c.x + r * 1.02 * a.cos(), c.y + r * 1.02 * a.sin()),
                    ],
                    stroke,
                );
            }
        }
        Icon::External => {
            let r = s * 0.35;
            let box_pts = vec![
                pos2(c.x - r * 0.1, c.y - r * 0.85),
                pos2(c.x - r * 0.85, c.y - r * 0.85),
                pos2(c.x - r * 0.85, c.y + r * 0.85),
                pos2(c.x + r * 0.85, c.y + r * 0.85),
                pos2(c.x + r * 0.85, c.y + r * 0.1),
            ];
            painter.add(Shape::line(box_pts, stroke));
            let tip = pos2(c.x + r * 0.85, c.y - r * 0.85);
            painter.line_segment([pos2(c.x - r * 0.05, c.y + r * 0.05), tip], stroke);
            painter.line_segment([pos2(tip.x - r * 0.55, tip.y), tip], stroke);
            painter.line_segment([pos2(tip.x, tip.y + r * 0.55), tip], stroke);
        }
        Icon::Search => {
            let r = s * 0.27;
            let lens = pos2(c.x - r * 0.25, c.y - r * 0.25);
            painter.circle_stroke(lens, r, stroke);
            painter.line_segment(
                [
                    pos2(lens.x + r * 0.72, lens.y + r * 0.72),
                    pos2(c.x + r * 1.15, c.y + r * 1.15),
                ],
                stroke,
            );
        }
        Icon::Close => {
            let r = s * 0.28;
            painter.line_segment([pos2(c.x - r, c.y - r), pos2(c.x + r, c.y + r)], stroke);
            painter.line_segment([pos2(c.x - r, c.y + r), pos2(c.x + r, c.y - r)], stroke);
        }
        Icon::Refresh => {
            let r = s * 0.34;
            paint_arc(painter, c, r, -2.4, 2.2, stroke);
            let tip = pos2(c.x + r * 2.2_f32.cos(), c.y + r * 2.2_f32.sin());
            painter.line_segment([pos2(tip.x + r * 0.35, tip.y - r * 0.05), tip], stroke);
            painter.line_segment([pos2(tip.x - r * 0.05, tip.y - r * 0.38), tip], stroke);
        }
        Icon::ArrowUp => {
            let r = s * 0.3;
            painter.line_segment([pos2(c.x, c.y + r), pos2(c.x, c.y - r)], stroke);
            painter.line_segment([pos2(c.x - r * 0.75, c.y - r * 0.15), pos2(c.x, c.y - r)], stroke);
            painter.line_segment([pos2(c.x + r * 0.75, c.y - r * 0.15), pos2(c.x, c.y - r)], stroke);
        }
        Icon::ArrowDown => {
            let r = s * 0.3;
            painter.line_segment([pos2(c.x, c.y - r), pos2(c.x, c.y + r)], stroke);
            painter.line_segment([pos2(c.x - r * 0.75, c.y + r * 0.15), pos2(c.x, c.y + r)], stroke);
            painter.line_segment([pos2(c.x + r * 0.75, c.y + r * 0.15), pos2(c.x, c.y + r)], stroke);
        }
        Icon::Device => {
            let r = s * 0.34;
            let box_rect = snap_rect(Rect::from_center_size(c, vec2(r * 1.45, r * 2.0)), ppp);
            painter.rect_stroke(box_rect, 2.2, stroke, StrokeKind::Middle);
            painter.circle_filled(pos2(c.x, c.y - r * 0.45), r * 0.16, color);
            painter.circle_stroke(pos2(c.x, c.y + r * 0.32), r * 0.34, stroke);
        }
        Icon::Visualizer => {
            let r = s * 0.38;
            let heights = [0.35_f32, 0.72, 1.0, 0.62, 0.85];
            let bar_w = snap_px((s * 0.10).max(1.5), ppp);
            let gap = snap_px((s * 0.075).max(1.2), ppp);
            let total_w = heights.len() as f32 * bar_w + (heights.len() - 1) as f32 * gap;
            let start_x = snap_px(c.x - total_w * 0.5, ppp);
            let bottom = snap_px(c.y + r, ppp);
            for (i, h) in heights.iter().enumerate() {
                let bh = snap_px(r * 1.9 * h, ppp);
                let x = snap_px(start_x + i as f32 * (bar_w + gap), ppp);
                painter.rect_filled(
                    Rect::from_min_max(pos2(x, bottom - bh), pos2(x + bar_w, bottom)),
                    0.8,
                    color,
                );
            }
        }
        Icon::Palette => {
            let r = s * 0.37;
            painter.circle_stroke(c, r, stroke);
            painter.circle_filled(pos2(c.x - r * 0.35, c.y - r * 0.25), r * 0.16, color);
            painter.circle_filled(pos2(c.x + r * 0.15, c.y - r * 0.4), r * 0.16, color);
            painter.circle_filled(pos2(c.x + r * 0.42, c.y + r * 0.05), r * 0.16, color);
        }
        Icon::Check => {
            let r = s * 0.34;
            painter.line_segment(
                [pos2(c.x - r * 0.85, c.y + r * 0.05), pos2(c.x - r * 0.15, c.y + r * 0.72)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x - r * 0.15, c.y + r * 0.72), pos2(c.x + r * 0.9, c.y - r * 0.65)],
                stroke,
            );
        }
        Icon::Plugin => {
            let r = s * 0.34;
            let body = snap_rect(
                Rect::from_min_max(pos2(c.x - r * 0.75, c.y - r * 0.15), pos2(c.x + r * 0.75, c.y + r * 0.65)),
                ppp,
            );
            painter.rect_stroke(body, 2.0, stroke, StrokeKind::Middle);
            painter.line_segment(
                [pos2(c.x - r * 0.35, c.y - r * 0.85), pos2(c.x - r * 0.35, body.top())],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x + r * 0.35, c.y - r * 0.85), pos2(c.x + r * 0.35, body.top())],
                stroke,
            );
            painter.line_segment([pos2(c.x, body.bottom()), pos2(c.x, c.y + r * 0.98)], stroke);
        }
        Icon::Sliders => {
            let r = s * 0.36;
            for (dy, knob_x) in [(-0.55_f32, -0.25_f32), (0.0, 0.35), (0.55, -0.1)] {
                let y = snap_px(c.y + r * dy, ppp);
                painter.line_segment([pos2(c.x - r * 0.9, y), pos2(c.x + r * 0.9, y)], stroke);
                painter.circle_filled(pos2(c.x + r * knob_x, y), (s * 0.11).max(1.8), color);
            }
        }
        Icon::Compact => {
            // Outer window frame with compact mini-player bar highlighted at bottom
            let r = s * 0.37;
            let outer = snap_rect(Rect::from_center_size(c, vec2(r * 2.0, r * 1.75)), ppp);
            painter.rect_stroke(outer, 2.0, stroke, StrokeKind::Middle);
            let mini = snap_rect(
                Rect::from_min_max(
                    pos2(outer.left() + r * 0.32, outer.center().y + r * 0.05),
                    pos2(outer.right() - r * 0.32, outer.bottom() - r * 0.28),
                ),
                ppp,
            );
            painter.rect_filled(mini, 1.0, color);
        }
        Icon::Expand => {
            // Full window with sidebar + library grid
            let r = s * 0.37;
            let outer = snap_rect(Rect::from_center_size(c, vec2(r * 2.0, r * 1.75)), ppp);
            painter.rect_stroke(outer, 2.0, stroke, StrokeKind::Middle);
            let split_x = snap_px(outer.left() + outer.width() * 0.36, ppp);
            painter.line_segment([pos2(split_x, outer.top()), pos2(split_x, outer.bottom())], stroke);
        }
        Icon::Lyrics => {
            // Clean speech/lyric lines with subtle quote rhythm
            let r = s * 0.36;
            for (dy, w_frac) in [(-0.55_f32, 0.90_f32), (-0.05, 0.72), (0.45, 0.52)] {
                let y = snap_px(c.y + r * dy, ppp);
                painter.line_segment(
                    [pos2(snap_px(c.x - r * 0.85, ppp), y), pos2(snap_px(c.x - r * 0.85 + r * 1.75 * w_frac, ppp), y)],
                    stroke,
                );
            }
        }
        Icon::StarFilled => {
            let r_out = s * 0.40;
            let r_in = s * 0.17;
            let mut pts = Vec::with_capacity(10);
            for i in 0..10 {
                let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                let r = if i % 2 == 0 { r_out } else { r_in };
                pts.push(pos2(c.x + r * angle.cos(), c.y + r * angle.sin()));
            }
            for i in 0..10 {
                let next = (i + 1) % 10;
                painter.add(Shape::convex_polygon(vec![c, pts[i], pts[next]], color, Stroke::NONE));
            }
        }
        Icon::Grid => {
            let r = s * 0.36;
            let cell = snap_px(r * 0.78, ppp);
            let off = snap_px(r * 0.52, ppp);
            for dx in [-off, off] {
                for dy in [-off, off] {
                    let cr = snap_rect(Rect::from_center_size(pos2(c.x + dx, c.y + dy), Vec2::splat(cell)), ppp);
                    painter.rect_stroke(cr, 1.0, stroke, StrokeKind::Inside);
                }
            }
        }
    }
}

fn paint_arc(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
    stroke: Stroke,
) {
    let steps = 14;
    let pts: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let a = start_angle + (end_angle - start_angle) * t;
            pos2(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect();
    painter.add(Shape::line(pts, stroke));
}

/// Uniform crisp aligned checkbox (exact 15x15px square, centered inside `cell_rect`).
pub fn paint_checkbox(
    painter: &Painter,
    cell_rect: Rect,
    checked: bool,
    indeterminate: bool,
    hovered: bool,
    palette: &Palette,
) {
    let ppp = painter.pixels_per_point().max(1.0);
    let box_rect = snap_rect(Rect::from_center_size(cell_rect.center(), Vec2::splat(15.0)), ppp);
    let (fill, border) = if checked || indeterminate {
        (palette.accent, palette.accent)
    } else if hovered {
        (palette.hover, palette.accent)
    } else {
        (palette.surface, palette.border_strong)
    };
    painter.rect_filled(box_rect, 3.5, fill);
    painter.rect_stroke(box_rect, 3.5, Stroke::new(1.0, border), StrokeKind::Inside);

    if checked {
        let c = box_rect.center();
        let stroke = Stroke::new(1.8, palette.on_accent());
        painter.line_segment([pos2(c.x - 3.6, c.y + 0.2), pos2(c.x - 1.0, c.y + 3.0)], stroke);
        painter.line_segment([pos2(c.x - 1.0, c.y + 3.0), pos2(c.x + 3.8, c.y - 2.8)], stroke);
    } else if indeterminate {
        let c = box_rect.center();
        painter.line_segment(
            [pos2(c.x - 3.5, c.y), pos2(c.x + 3.5, c.y)],
            Stroke::new(1.8, palette.on_accent()),
        );
    }
}

/// Paints a crisp 3-bar playing indicator that reacts to real audio energy when playing.
pub fn paint_playing_bars(
    painter: &Painter,
    rect: Rect,
    time: f64,
    playing: bool,
    audio_level: f32,
    color: Color32,
) {
    let ppp = painter.pixels_per_point().max(1.0);
    let w = rect.width();
    let h = rect.height();
    let bar_w = snap_px((w * 0.24).max(2.0), ppp);
    let gap = snap_px((w - bar_w * 3.0) / 2.0, ppp);
    let phases = [0.0_f64, 2.0, 4.1];
    let speeds = [5.8_f64, 7.6, 4.9];
    let drive = if audio_level > 0.02 {
        (0.35 + audio_level * 0.85).clamp(0.35, 1.0)
    } else {
        0.55
    };

    for i in 0..3 {
        let level = if playing {
            let wave = ((time * speeds[i] + phases[i]).sin() * 0.5 + 0.5) as f32;
            (0.20 + 0.80 * wave * drive).clamp(0.18, 1.0)
        } else {
            0.24
        };
        let bh = snap_px((h * level).max(2.0), ppp);
        let x = snap_px(rect.left() + i as f32 * (bar_w + gap), ppp);
        let bottom = snap_px(rect.bottom(), ppp);
        let bar = Rect::from_min_max(pos2(x, bottom - bh), pos2(x + bar_w, bottom));
        painter.rect_filled(bar, 0.8, color);
    }
}

/// Paints the unified TxPlayer visualizer emblem (fallback if PNG texture unavailable).
pub fn paint_brand_mark(painter: &Painter, rect: Rect, palette: &Palette) {
    let rounding = (rect.width() * 0.22).clamp(6.0, 14.0);
    let bg = if palette.is_light {
        Color32::from_rgb(17, 23, 26)
    } else {
        Color32::from_rgb(14, 19, 22)
    };
    painter.rect_filled(rect, rounding, bg);
    painter.rect_stroke(
        rect,
        rounding,
        Stroke::new(1.0, palette.accent.gamma_multiply(0.45)),
        StrokeKind::Inside,
    );

    let inner = rect.shrink(rect.width() * 0.16);
    let heights = [0.33_f32, 0.56, 0.82, 0.67, 1.0, 0.77, 0.89, 0.50, 0.28];
    let n = heights.len();
    let gap = (inner.width() * 0.045).max(0.8);
    let bar_w = ((inner.width() - gap * (n - 1) as f32) / n as f32).max(1.4);
    let horizon = inner.bottom() - inner.height() * 0.24;
    let max_bar_h = (horizon - inner.top() - inner.height() * 0.08).max(4.0);

    for (i, &h) in heights.iter().enumerate() {
        let x = inner.left() + i as f32 * (bar_w + gap);
        let bh = (max_bar_h * h).max(2.0);
        let color = if i == 4 || i == 6 {
            palette.hires_gold
        } else if i == 2 {
            palette.accent_bright
        } else {
            palette.accent
        };
        let top_y = horizon - bh;
        painter.rect_filled(
            Rect::from_min_max(pos2(x, top_y), pos2(x + bar_w, horizon)),
            bar_w * 0.48,
            color,
        );
    }
}

/// Paints a sleek minimalist vinyl-disc fallback cover when a track has no artwork.
pub fn paint_fallback_cover(painter: &Painter, rect: Rect, seed: u64, title: &str) {
    const PALETTE: &[(Color32, Color32)] = &[
        (Color32::from_rgb(24, 46, 42), Color32::from_rgb(86, 214, 162)),
        (Color32::from_rgb(30, 38, 56), Color32::from_rgb(112, 168, 240)),
        (Color32::from_rgb(48, 36, 28), Color32::from_rgb(232, 180, 102)),
        (Color32::from_rgb(44, 30, 48), Color32::from_rgb(198, 134, 224)),
        (Color32::from_rgb(26, 44, 48), Color32::from_rgb(94, 202, 212)),
        (Color32::from_rgb(48, 32, 34), Color32::from_rgb(228, 124, 130)),
    ];
    let (bg, fg) = PALETTE[(seed as usize) % PALETTE.len()];
    let rounding = (rect.width() * 0.14).clamp(4.0, 10.0);
    painter.rect_filled(rect, rounding, bg);

    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.42;

    if rect.width() >= 36.0 {
        painter.circle_filled(c, r, Color32::from_rgba_unmultiplied(10, 12, 13, 120));
        painter.circle_stroke(
            c,
            r * 0.84,
            Stroke::new(0.8, Color32::from_rgba_unmultiplied(255, 255, 255, 22)),
        );
        painter.circle_stroke(
            c,
            r * 0.62,
            Stroke::new(0.8, Color32::from_rgba_unmultiplied(255, 255, 255, 18)),
        );
        painter.circle_filled(c, r * 0.36, fg.gamma_multiply(0.85));
        painter.circle_filled(c, r * 0.11, bg);
    } else {
        let ch = title
            .chars()
            .find(|ch| ch.is_alphanumeric())
            .unwrap_or('T')
            .to_ascii_uppercase()
            .to_string();
        painter.text(
            c,
            Align2::CENTER_CENTER,
            ch,
            FontId::proportional(rect.height() * 0.48),
            fg,
        );
    }

    painter.rect_stroke(
        rect,
        rounding,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18)),
        StrokeKind::Inside,
    );
}

/// Compact reusable vector icon button with hover background pill.
#[allow(clippy::too_many_arguments)]
pub fn icon_button(
    ui: &mut egui::Ui,
    size: Vec2,
    icon: Icon,
    icon_size: f32,
    color: Color32,
    hover_color: Color32,
    hover_bg: Color32,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if hovered {
            ui.painter().rect_filled(rect, 5.0, hover_bg);
        }
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(icon_size));
        paint_icon(
            ui.painter(),
            icon_rect,
            icon,
            if hovered { hover_color } else { color },
        );
    }
    response.on_hover_text(tooltip)
}

/// Paints a single crisp vector 5-pointed star (font-independent across Windows/macOS/Linux!).
/// `fill_state`: `0` = empty outline, `1` = left half filled, `2` = fully filled.
pub fn paint_star(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    fill_state: u8,
    fill_color: Color32,
    empty_color: Color32,
) {
    let mut pts = Vec::with_capacity(10);
    let inner_r = radius * 0.42;
    for i in 0..10 {
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * (std::f32::consts::PI / 5.0);
        let r = if i % 2 == 0 { radius } else { inner_r };
        pts.push(pos2(center.x + r * angle.cos(), center.y + r * angle.sin()));
    }

    // Triangulate star around center so concave polygon fills crisply in egui
    let fill_tri = |p: &Painter, a: Pos2, b: Pos2, col: Color32| {
        p.add(Shape::convex_polygon(vec![center, a, b], col, Stroke::NONE));
    };

    if fill_state >= 2 {
        for i in 0..10 {
            fill_tri(painter, pts[i], pts[(i + 1) % 10], fill_color);
        }
    } else if fill_state == 1 {
        // Left half of the 5-pointed star: vertices 5..=9 -> 0
        for i in 5..10 {
            fill_tri(painter, pts[i], pts[(i + 1) % 10], fill_color);
        }
    }

    let stroke_col = if fill_state > 0 { fill_color } else { empty_color };
    let mut outline = pts;
    outline.push(outline[0]);
    painter.add(Shape::line(outline, Stroke::new(1.0, stroke_col)));
}

/// Paints a compact read-only row of 5 vector stars for a half-star rating (`0..=10`).
pub fn paint_star_row(
    painter: &Painter,
    left_center: Pos2,
    rating_half_stars: u8,
    star_radius: f32,
    spacing: f32,
    fill_color: Color32,
    empty_color: Color32,
) {
    let clamped = rating_half_stars.min(10);
    for i in 0..5u8 {
        let threshold_half = i * 2 + 1;
        let threshold_full = i * 2 + 2;
        let state = if clamped >= threshold_full {
            2
        } else if clamped >= threshold_half {
            1
        } else {
            0
        };
        let cx = left_center.x + star_radius + i as f32 * (star_radius * 2.0 + spacing);
        paint_star(
            painter,
            pos2(cx, left_center.y),
            star_radius,
            state,
            fill_color,
            empty_color,
        );
    }
}

/// Interactive 5-star widget supporting half-star precision (`0..=10`, i.e. `0.0` to `5.0` in `0.5` steps).
/// Returns `Some(new_half_stars)` when clicked (clicking the current rating clears to `0`).
pub fn interactive_half_star_rating(
    ui: &mut egui::Ui,
    current_half_stars: u8,
    star_radius: f32,
    spacing: f32,
    fill_color: Color32,
    empty_color: Color32,
) -> Option<u8> {
    let step_w = star_radius * 2.0 + spacing;
    let total_w = star_radius * 2.0 * 5.0 + spacing * 4.0 + 2.0;
    let total_h = (star_radius * 2.0 + 4.0).max(16.0);
    let (rect, response) = ui.allocate_exact_size(vec2(total_w, total_h), egui::Sense::click());

    let hover_rating = if response.hovered()
        && let Some(pos) = ui.ctx().pointer_hover_pos()
    {
        let rel_x = (pos.x - rect.left()).clamp(0.0, total_w);
        let star_idx = ((rel_x / step_w).floor() as u8).min(4);
        let in_star = rel_x - star_idx as f32 * step_w;
        let is_left_half = in_star < star_radius;
        Some((star_idx * 2 + if is_left_half { 1 } else { 2 }).min(10))
    } else {
        None
    };

    if ui.is_rect_visible(rect) {
        let display = hover_rating.unwrap_or(current_half_stars.min(10));
        paint_star_row(
            ui.painter(),
            pos2(rect.left() + 1.0, rect.center().y),
            display,
            star_radius,
            spacing,
            fill_color,
            empty_color,
        );
    }

    let clicked_rating = if response.clicked() {
        let target = hover_rating.unwrap_or(current_half_stars);
        if target == current_half_stars {
            Some(0)
        } else {
            Some(target)
        }
    } else {
        None
    };

    let tip = match hover_rating {
        Some(h) => format!("Set rating: {:.1} / 5.0 (click again to clear)", h as f32 * 0.5),
        None if current_half_stars > 0 => format!(
            "Rating: {:.1} / 5.0 · Click left/right half of any star",
            current_half_stars as f32 * 0.5
        ),
        None => "Click left/right half of a star to rate (0.5–5.0)".to_string(),
    };
    response.on_hover_text(tip);

    clicked_rating
}
