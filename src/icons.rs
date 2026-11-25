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
}

pub fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let stroke = Stroke::new((s * 0.11).clamp(1.2, 2.0), color);

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
            let w = (s * 0.22).max(2.5);
            let h = s * 0.64;
            let gap = s * 0.14;
            let left = Rect::from_center_size(pos2(c.x - gap - w * 0.5, c.y), vec2(w, h));
            let right = Rect::from_center_size(pos2(c.x + gap + w * 0.5, c.y), vec2(w, h));
            painter.rect_filled(left, 1.5, color);
            painter.rect_filled(right, 1.5, color);
        }
        Icon::Prev => {
            let r = s * 0.32;
            let bar = Rect::from_center_size(pos2(c.x - r * 0.85, c.y), vec2(2.0, r * 1.9));
            painter.rect_filled(bar, 1.0, color);
            let pts = vec![
                pos2(c.x + r * 0.9, c.y - r * 0.95),
                pos2(c.x - r * 0.55, c.y),
                pos2(c.x + r * 0.9, c.y + r * 0.95),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Next => {
            let r = s * 0.32;
            let bar = Rect::from_center_size(pos2(c.x + r * 0.85, c.y), vec2(2.0, r * 1.9));
            painter.rect_filled(bar, 1.0, color);
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
            let loop_rect = Rect::from_center_size(c, vec2(r * 2.0, r * 1.25));
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
        Icon::HeartOutline | Icon::HeartFilled => {
            let r = s * 0.34;
            let pts = heart_points(c, r);
            if icon == Icon::HeartFilled {
                painter.add(Shape::closed_line(pts.clone(), Stroke::new(1.2, color)));
                let fill_center = pos2(c.x, c.y + r * 0.1);
                for i in 0..pts.len() {
                    let next = (i + 1) % pts.len();
                    painter.add(Shape::convex_polygon(
                        vec![fill_center, pts[i], pts[next]],
                        color,
                        Stroke::NONE,
                    ));
                }
            } else {
                painter.add(Shape::closed_line(pts, stroke));
            }
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
            // Stacked books / library bars
            let bw = (s * 0.15).max(1.8);
            for (i, h) in [0.75_f32, 0.95, 0.82].iter().enumerate() {
                let x = c.x - r * 0.85 + i as f32 * (bw + s * 0.13);
                let bh = r * 1.9 * h;
                painter.rect_filled(
                    Rect::from_min_max(pos2(x, c.y + r * 0.85 - bh), pos2(x + bw, c.y + r * 0.85)),
                    1.0,
                    color,
                );
            }
        }
        Icon::Queue => {
            let r = s * 0.35;
            for offset in [-0.55_f32, 0.0, 0.55] {
                let y = c.y + r * offset;
                painter.line_segment([pos2(c.x - r * 0.9, y), pos2(c.x + r * 0.9, y)], stroke);
            }
        }
        Icon::Playlist => {
            let r = s * 0.36;
            painter.line_segment(
                [pos2(c.x - r * 0.9, c.y - r * 0.55), pos2(c.x + r * 0.35, c.y - r * 0.55)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x - r * 0.9, c.y), pos2(c.x + r * 0.15, c.y)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x - r * 0.9, c.y + r * 0.55), pos2(c.x + r * 0.15, c.y + r * 0.55)],
                stroke,
            );
            // Small note on right
            let head = pos2(c.x + r * 0.55, c.y + r * 0.45);
            painter.circle_filled(head, r * 0.24, color);
            painter.line_segment(
                [pos2(head.x + r * 0.18, head.y), pos2(head.x + r * 0.18, c.y - r * 0.55)],
                stroke,
            );
        }
        Icon::Folder | Icon::FolderPlus => {
            let r = s * 0.36;
            let body = Rect::from_min_max(
                pos2(c.x - r, c.y - r * 0.45),
                pos2(c.x + r, c.y + r * 0.8),
            );
            painter.rect_stroke(body, 2.0, stroke, StrokeKind::Middle);
            let tab_pts = vec![
                pos2(c.x - r, c.y - r * 0.45),
                pos2(c.x - r, c.y - r * 0.82),
                pos2(c.x - r * 0.18, c.y - r * 0.82),
                pos2(c.x + r * 0.05, c.y - r * 0.45),
            ];
            painter.add(Shape::line(tab_pts, stroke));
            if icon == Icon::FolderPlus {
                let pr = r * 0.35;
                let pc = pos2(c.x, c.y + r * 0.18);
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
            let body = Rect::from_min_max(
                pos2(c.x - r * 0.62, c.y - r * 0.55),
                pos2(c.x + r * 0.62, c.y + r * 0.85),
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
                pos2(c.x - r * 0.85, c.y - r * 0.85),
                pos2(c.x + r * 0.05, c.y - r * 0.85),
                pos2(c.x + r * 0.88, c.y + r * 0.02),
                pos2(c.x + r * 0.02, c.y + r * 0.88),
                pos2(c.x - r * 0.85, c.y + r * 0.05),
            ];
            painter.add(Shape::closed_line(pts, stroke));
            painter.circle_filled(pos2(c.x - r * 0.4, c.y - r * 0.4), (s * 0.08).max(1.2), color);
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
            let box_rect = Rect::from_center_size(c, vec2(r * 1.45, r * 2.0));
            painter.rect_stroke(box_rect, 2.5, stroke, StrokeKind::Middle);
            painter.circle_filled(pos2(c.x, c.y - r * 0.45), r * 0.15, color);
            painter.circle_stroke(pos2(c.x, c.y + r * 0.32), r * 0.35, stroke);
        }
        Icon::Visualizer => {
            let r = s * 0.38;
            let heights = [0.35_f32, 0.72, 1.0, 0.62, 0.85];
            let bar_w = (s * 0.10).max(1.5);
            let gap = (s * 0.075).max(1.2);
            let total_w = heights.len() as f32 * bar_w + (heights.len() - 1) as f32 * gap;
            let start_x = c.x - total_w * 0.5;
            let bottom = c.y + r;
            for (i, h) in heights.iter().enumerate() {
                let bh = r * 1.9 * h;
                let x = start_x + i as f32 * (bar_w + gap);
                painter.rect_filled(
                    Rect::from_min_max(pos2(x, bottom - bh), pos2(x + bar_w, bottom)),
                    1.0,
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
            let body = Rect::from_min_max(
                pos2(c.x - r * 0.75, c.y - r * 0.15),
                pos2(c.x + r * 0.75, c.y + r * 0.65),
            );
            painter.rect_stroke(body, 2.0, stroke, StrokeKind::Middle);
            // Two top prongs
            painter.line_segment(
                [pos2(c.x - r * 0.35, c.y - r * 0.85), pos2(c.x - r * 0.35, c.y - r * 0.15)],
                stroke,
            );
            painter.line_segment(
                [pos2(c.x + r * 0.35, c.y - r * 0.85), pos2(c.x + r * 0.35, c.y - r * 0.15)],
                stroke,
            );
            // Cord stem
            painter.line_segment([pos2(c.x, c.y + r * 0.65), pos2(c.x, c.y + r * 0.98)], stroke);
        }
        Icon::Sliders => {
            let r = s * 0.36;
            for (dy, knob_x) in [(-0.55_f32, -0.25_f32), (0.0, 0.35), (0.55, -0.1)] {
                let y = c.y + r * dy;
                painter.line_segment([pos2(c.x - r * 0.9, y), pos2(c.x + r * 0.9, y)], stroke);
                painter.circle_filled(pos2(c.x + r * knob_x, y), (s * 0.11).max(1.8), color);
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

fn heart_points(center: Pos2, r: f32) -> Vec<Pos2> {
    let steps = 28;
    let scale = r / 15.5;
    (0..steps)
        .map(|i| {
            let t = (i as f32 / steps as f32) * std::f32::consts::TAU;
            let x = 16.0 * t.sin().powi(3);
            let y = -(13.0 * t.cos()
                - 5.0 * (2.0 * t).cos()
                - 2.0 * (3.0 * t).cos()
                - (4.0 * t).cos());
            pos2(center.x + x * scale, center.y + y * scale - r * 0.08)
        })
        .collect()
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
    let box_rect = Rect::from_center_size(cell_rect.center(), Vec2::splat(15.0));
    let (fill, border) = if checked || indeterminate {
        (palette.accent, palette.accent)
    } else if hovered {
        (palette.hover, palette.accent)
    } else {
        (palette.surface, palette.border_strong)
    };
    painter.rect_filled(box_rect, 3.5, fill);
    painter.rect_stroke(box_rect, 3.5, Stroke::new(1.1, border), StrokeKind::Inside);

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

/// Paints an animated 3-bar studio equalizer indicator for the currently playing track.
pub fn paint_playing_bars(painter: &Painter, rect: Rect, time: f64, playing: bool, color: Color32) {
    let w = rect.width();
    let h = rect.height();
    let bar_w = (w * 0.22).max(2.0);
    let gap = (w - bar_w * 3.0) / 2.0;
    let phases = [0.0_f64, 2.1, 4.3];
    let speeds = [7.2_f64, 9.5, 6.1];

    for i in 0..3 {
        let level = if playing {
            let wave = ((time * speeds[i] + phases[i]).sin() * 0.5 + 0.5) as f32;
            let wave2 = ((time * (speeds[i] * 0.57) + phases[i] * 1.7).cos() * 0.5 + 0.5) as f32;
            0.22 + 0.78 * (wave * 0.65 + wave2 * 0.35)
        } else {
            0.24
        };
        let bh = (h * level).max(2.5);
        let x = rect.left() + i as f32 * (bar_w + gap);
        let bar = Rect::from_min_max(pos2(x, rect.bottom() - bh), pos2(x + bar_w, rect.bottom()));
        painter.rect_filled(bar, 1.0, color);
    }
}

/// Paints the unified TxPlayer visualizer emblem (matches AppIcon.svg — 9 precision bars + peak caps + reflection).
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
    // Exact 9-bar proportions from assets/AppIcon.svg
    let heights = [0.33_f32, 0.56, 0.82, 0.67, 1.0, 0.77, 0.89, 0.50, 0.28];
    let n = heights.len();
    let gap = (inner.width() * 0.045).max(0.8);
    let bar_w = ((inner.width() - gap * (n - 1) as f32) / n as f32).max(1.4);
    let horizon = inner.bottom() - inner.height() * 0.24;
    let max_bar_h = (horizon - inner.top() - inner.height() * 0.08).max(4.0);

    painter.line_segment(
        [pos2(inner.left(), horizon + 1.0), pos2(inner.right(), horizon + 1.0)],
        Stroke::new(0.8, palette.accent.gamma_multiply(0.32)),
    );

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
        // Floating peak caps on bars 2, 4, 6 (matching AppIcon.svg)
        if matches!(i, 2 | 4 | 6) {
            let cap_h = (bar_w * 0.45).clamp(1.0, 2.2);
            let cap_y = (top_y - cap_h - 1.4).max(inner.top());
            painter.rect_filled(
                Rect::from_min_max(pos2(x, cap_y), pos2(x + bar_w, cap_y + cap_h)),
                cap_h * 0.5,
                color,
            );
        }
        // Soft acoustic reflection below horizon
        let refl_h = (inner.height() * 0.20 * h).max(1.2);
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x, horizon + 2.0),
                pos2(x + bar_w, (horizon + 2.0 + refl_h).min(inner.bottom())),
            ),
            bar_w * 0.4,
            Color32::from_rgba_unmultiplied(palette.accent.r(), palette.accent.g(), palette.accent.b(), 65),
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
            ui.painter().rect_filled(rect, 6.0, hover_bg);
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
