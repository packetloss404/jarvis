//! Layout calculation — recursive tree-to-rect computation.

use crate::tree::{Direction, SplitNode};
use jarvis_common::types::Rect;

use super::LayoutEngine;

impl LayoutEngine {
    pub fn compute(&self, root: &SplitNode, bounds: Rect) -> Vec<(u32, Rect)> {
        let pad = self.outer_padding as f64;
        let inset = Rect {
            x: bounds.x + pad,
            y: bounds.y + pad,
            width: (bounds.width - pad * 2.0).max(0.0),
            height: (bounds.height - pad * 2.0).max(0.0),
        };
        let mut results = Vec::new();
        self.layout_node(root, inset, &mut results);
        results
    }

    fn layout_node(&self, node: &SplitNode, bounds: Rect, out: &mut Vec<(u32, Rect)>) {
        match node {
            SplitNode::Leaf { pane_id } => {
                out.push((*pane_id, bounds));
            }
            SplitNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let gap = self.gap as f64;
                let min = self.min_pane_size;
                let (a, b) = match direction {
                    Direction::Horizontal => {
                        let available_width = (bounds.width - gap).max(0.0);
                        // ISS-43: clamp ratio so neither child is narrower than
                        // min_pane_size pixels.
                        let clamped_ratio = if available_width > 0.0 {
                            let min_ratio = min / available_width;
                            ratio.clamp(min_ratio, 1.0 - min_ratio)
                        } else {
                            *ratio
                        };
                        let w1 = available_width * clamped_ratio;
                        let w2 = (available_width - w1).max(0.0);
                        (
                            Rect {
                                x: bounds.x,
                                y: bounds.y,
                                width: w1,
                                height: bounds.height,
                            },
                            Rect {
                                x: bounds.x + w1 + gap,
                                y: bounds.y,
                                width: w2,
                                height: bounds.height,
                            },
                        )
                    }
                    Direction::Vertical => {
                        let available_height = (bounds.height - gap).max(0.0);
                        // ISS-43: clamp ratio so neither child is shorter than
                        // min_pane_size pixels.
                        let clamped_ratio = if available_height > 0.0 {
                            let min_ratio = min / available_height;
                            ratio.clamp(min_ratio, 1.0 - min_ratio)
                        } else {
                            *ratio
                        };
                        let h1 = available_height * clamped_ratio;
                        let h2 = (available_height - h1).max(0.0);
                        (
                            Rect {
                                x: bounds.x,
                                y: bounds.y,
                                width: bounds.width,
                                height: h1,
                            },
                            Rect {
                                x: bounds.x,
                                y: bounds.y + h1 + gap,
                                width: bounds.width,
                                height: h2,
                            },
                        )
                    }
                };
                self.layout_node(first, a, out);
                self.layout_node(second, b, out);
            }
        }
    }
}
