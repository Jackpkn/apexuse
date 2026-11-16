use crate::driver::{OcrBox, Rect, UiElement};
use crate::json::JsonValue;

#[derive(Debug, Clone, PartialEq)]
pub struct VisualMark {
    pub id: String,
    pub source: String, // "ax" or "ocr"
    pub role: String,
    pub label: String,
    pub ax_ref: Option<String>,
    pub frame: Rect,
    pub center_x: f64,
    pub center_y: f64,
    pub prefer_ax: bool,
}

impl VisualMark {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = vec![
            ("id", JsonValue::String(self.id.clone())),
            ("source", JsonValue::String(self.source.clone())),
            ("role", JsonValue::String(self.role.clone())),
            ("label", JsonValue::String(self.label.clone())),
            (
                "center",
                JsonValue::object(vec![
                    ("x", JsonValue::Number(round1(self.center_x))),
                    ("y", JsonValue::Number(round1(self.center_y))),
                ]),
            ),
            (
                "frame",
                JsonValue::object(vec![
                    ("x", JsonValue::Number(round1(self.frame.x))),
                    ("y", JsonValue::Number(round1(self.frame.y))),
                    ("w", JsonValue::Number(round1(self.frame.w))),
                    ("h", JsonValue::Number(round1(self.frame.h))),
                ]),
            ),
        ];
        if let Some(ref axr) = self.ax_ref {
            entries.push(("ax_ref", JsonValue::String(axr.clone())));
        }
        JsonValue::object(entries)
    }

    pub fn overlay_json(&self) -> JsonValue {
        JsonValue::object(vec![
            ("id", JsonValue::String(self.id.clone())),
            ("source", JsonValue::String(self.source.clone())),
            ("x", JsonValue::Number(self.frame.x)),
            ("y", JsonValue::Number(self.frame.y)),
            ("w", JsonValue::Number(self.frame.w)),
            ("h", JsonValue::Number(self.frame.h)),
        ])
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub fn rect_iou(a: &Rect, b: &Rect) -> f64 {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    let inter = (x1 - x0) * (y1 - y0);
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

pub fn rect_contains_center(outer: &Rect, inner: &Rect) -> bool {
    let cx = inner.x + inner.w * 0.5;
    let cy = inner.y + inner.h * 0.5;
    cx >= outer.x && cx <= outer.x + outer.w && cy >= outer.y && cy <= outer.y + outer.h
}

fn within_window(r: &Rect, win: &Rect) -> bool {
    let cx = r.x + r.w * 0.5;
    let cy = r.y + r.h * 0.5;
    r.w >= 6.0
        && r.h >= 6.0
        && cx >= win.x
        && cx <= win.x + win.w
        && cy >= win.y
        && cy <= win.y + win.h
}

pub fn supports_ax_press(el: &UiElement) -> bool {
    matches!(
        el.role.as_str(),
        "Button"
            | "PopUpButton"
            | "MenuButton"
            | "CheckBox"
            | "RadioButton"
            | "MenuItem"
            | "Link"
            | "DisclosureTriangle"
    ) && el.actions.iter().any(|a| a == "AXPress")
}

/// Fuses semantic Accessibility bounding boxes with Vision OCR detections into
/// spatially ordered Set-of-Mark targets (`m1`, `m2`, ...).
pub fn fuse_marks(
    win: &Rect,
    elements: &[UiElement],
    ocr_boxes: &[OcrBox],
    max_marks: usize,
) -> Vec<VisualMark> {
    let mut candidates: Vec<VisualMark> = Vec::new();
    let win_area = (win.w * win.h).max(1.0);

    for el in elements {
        if !el.interactive || el.enabled == Some(false) {
            continue;
        }
        let Some(frame) = el.frame.as_ref() else {
            continue;
        };
        if !within_window(frame, win) {
            continue;
        }
        // Skip large container frames (>75% of window), while preserving text editors.
        let is_text_input = matches!(el.role.as_str(), "TextArea" | "TextField" | "SearchField");
        if !is_text_input && (frame.w * frame.h) / win_area > 0.75 {
            continue;
        }
        // Deduplicate identical/nearly identical AX boxes
        if candidates.iter().any(|c| rect_iou(&c.frame, frame) >= 0.85) {
            continue;
        }
        let prefer_ax = supports_ax_press(el);

        candidates.push(VisualMark {
            id: String::new(),
            source: "ax".into(),
            role: el.role.clone(),
            label: el.label.clone(),
            ax_ref: Some(el.ref_id.clone()),
            frame: frame.clone(),
            center_x: frame.x + frame.w * 0.5,
            center_y: frame.y + frame.h * 0.5,
            prefer_ax,
        });
    }

    for ocr in ocr_boxes {
        let text = ocr.text.trim();
        if text.is_empty() || ocr.confidence < 0.45 {
            continue;
        }
        if !within_window(&ocr.frame, win) {
            continue;
        }
        if (ocr.frame.w * ocr.frame.h) / win_area > 0.50 {
            continue;
        }

        // Check if this OCR text corresponds to an existing semantic AX element
        if let Some(existing) = candidates
            .iter_mut()
            .find(|c| rect_iou(&c.frame, &ocr.frame) >= 0.25 || rect_contains_center(&c.frame, &ocr.frame))
        {
            if existing.label.is_empty() {
                existing.label = text.to_string();
            }
            continue;
        }

        // Avoid near-duplicate OCR boxes
        if candidates
            .iter()
            .any(|c| c.source == "ocr" && rect_iou(&c.frame, &ocr.frame) >= 0.65)
        {
            continue;
        }

        candidates.push(VisualMark {
            id: String::new(),
            source: "ocr".into(),
            role: "VisualText".into(),
            label: text.to_string(),
            ax_ref: None,
            frame: ocr.frame.clone(),
            center_x: ocr.frame.x + ocr.frame.w * 0.5,
            center_y: ocr.frame.y + ocr.frame.h * 0.5,
            prefer_ax: false,
        });
    }

    if candidates.len() > max_marks && max_marks > 0 {
        let mut tier0_inputs = Vec::new();
        let mut tier1_controls = Vec::new();
        let mut tier2_ax_other = Vec::new();
        let mut tier3_ocr = Vec::new();

        for c in candidates {
            if c.source == "ax"
                && matches!(
                    c.role.as_str(),
                    "TextField" | "TextArea" | "SearchField" | "ComboBox"
                )
            {
                tier0_inputs.push(c);
            } else if c.source == "ax"
                && (c.prefer_ax
                    || matches!(
                        c.role.as_str(),
                        "PopUpButton" | "MenuButton" | "TabGroup" | "Slider" | "SegmentedControl"
                    ))
            {
                tier1_controls.push(c);
            } else if c.source == "ax" {
                tier2_ax_other.push(c);
            } else {
                tier3_ocr.push(c);
            }
        }

        tier2_ax_other.sort_by_key(|c| c.label.trim().is_empty());
        tier3_ocr.sort_by(|a, b| {
            let row_a = (a.frame.y / 14.0).round() as i64;
            let row_b = (b.frame.y / 14.0).round() as i64;
            row_a
                .cmp(&row_b)
                .then_with(|| a.frame.x.partial_cmp(&b.frame.x).unwrap_or(std::cmp::Ordering::Equal))
        });

        let mut selected = Vec::with_capacity(max_marks);
        for bucket in [tier0_inputs, tier1_controls, tier2_ax_other] {
            for item in bucket {
                if selected.len() < max_marks {
                    selected.push(item);
                }
            }
        }
        let rem = max_marks.saturating_sub(selected.len());
        if rem > 0 && !tier3_ocr.is_empty() {
            if tier3_ocr.len() <= rem {
                selected.extend(tier3_ocr);
            } else {
                let len = tier3_ocr.len();
                let mut picked = vec![false; len];
                for k in 0..rem {
                    picked[(k * len) / rem] = true;
                }
                for (item, keep) in tier3_ocr.into_iter().zip(picked) {
                    if keep {
                        selected.push(item);
                    }
                }
            }
        }
        candidates = selected;
    }

    // Spatial reading order: band y into 14pt rows, then left-to-right
    candidates.sort_by(|a, b| {
        let row_a = (a.frame.y / 14.0).round() as i64;
        let row_b = (b.frame.y / 14.0).round() as i64;
        row_a
            .cmp(&row_b)
            .then_with(|| a.frame.x.partial_cmp(&b.frame.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    candidates.truncate(max_marks);
    for (idx, item) in candidates.iter_mut().enumerate() {
        item.id = format!("m{}", idx + 1);
    }
    candidates
}

pub fn format_mark_index(marks: &[VisualMark]) -> String {
    let mut lines = Vec::with_capacity(marks.len());
    for m in marks {
        let label = if m.label.is_empty() {
            String::new()
        } else {
            format!(" \"{}\"", m.label.replace('"', "'"))
        };
        let ax_part = m
            .ax_ref
            .as_ref()
            .map(|r| format!(" ref={r}"))
            .unwrap_or_default();
        lines.push(format!(
            "[{}] ({}) {}{} @({}, {})",
            m.id,
            m.source,
            m.role,
            label,
            m.center_x.round() as i64,
            m.center_y.round() as i64
        ) + &ax_part);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(r: &str, role: &str, label: &str, x: f64, y: f64, w: f64, h: f64) -> UiElement {
        UiElement {
            ref_id: r.into(),
            role: role.into(),
            label: label.into(),
            value: None,
            window: 0,
            depth: 2,
            interactive: true,
            enabled: Some(true),
            focused: false,
            actions: vec!["AXPress".into()],
            frame: Some(Rect { x, y, w, h }),
        }
    }

    #[test]
    fn fuses_ax_and_ocr_without_duplicating_overlapping_buttons() {
        let win = Rect {
            x: 100.0,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        let els = vec![
            elem("e1", "Button", "", 150.0, 200.0, 80.0, 32.0),
            elem("e2", "Button", "Cancel", 260.0, 200.0, 80.0, 32.0),
        ];
        let ocrs = vec![
            OcrBox {
                text: "Save".into(),
                confidence: 0.95,
                frame: Rect {
                    x: 162.0,
                    y: 206.0,
                    w: 50.0,
                    h: 18.0,
                },
            },
            OcrBox {
                text: "Canvas Action".into(),
                confidence: 0.90,
                frame: Rect {
                    x: 420.0,
                    y: 350.0,
                    w: 110.0,
                    h: 28.0,
                },
            },
        ];

        let marks = fuse_marks(&win, &els, &ocrs, 50);
        assert_eq!(marks.len(), 3);
        assert_eq!(marks[0].id, "m1");
        assert_eq!(marks[0].source, "ax");
        assert_eq!(marks[0].label, "Save"); // Enriched from overlapping OCR
        assert_eq!(marks[1].id, "m2");
        assert_eq!(marks[1].label, "Cancel");
        assert_eq!(marks[2].id, "m3");
        assert_eq!(marks[2].source, "ocr");
        assert_eq!(marks[2].label, "Canvas Action");
    }

    #[test]
    fn retains_large_textarea_as_visual_mark() {
        let win = Rect {
            x: 0.0,
            y: 0.0,
            w: 600.0,
            h: 500.0,
        };
        let editor = elem("e1", "TextArea", "", 10.0, 40.0, 580.0, 450.0);
        let marks = fuse_marks(&win, &[editor], &[], 50);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].role, "TextArea");
    }

    #[test]
    fn prioritizes_inputs_and_buttons_over_top_ocr_clutter_under_cap() {
        let win = Rect {
            x: 0.0,
            y: 0.0,
            w: 1200.0,
            h: 900.0,
        };
        let mut ocrs = Vec::new();
        for i in 0..30 {
            ocrs.push(OcrBox {
                text: format!("line_{i}"),
                confidence: 0.95,
                frame: Rect {
                    x: 40.0,
                    y: 40.0 + (i as f64) * 16.0,
                    w: 120.0,
                    h: 12.0,
                },
            });
        }
        let els = vec![
            elem("e1", "TextArea", "Prompt", 800.0, 820.0, 320.0, 48.0),
            elem("e2", "Button", "Send", 1130.0, 826.0, 32.0, 32.0),
        ];
        let marks = fuse_marks(&win, &els, &ocrs, 10);
        assert_eq!(marks.len(), 10);
        assert!(marks.iter().any(|m| m.role == "TextArea" && m.label == "Prompt"));
        assert!(marks.iter().any(|m| m.role == "Button" && m.label == "Send"));
        // Spatial order preserved: bottom Prompt/Send appear at the end of m1..m10
        assert!(marks.last().unwrap().center_y > 800.0);
    }
}
