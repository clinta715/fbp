//! The layout tree (SCOPE section 3): three node kinds over a fixed panel vocabulary.
//!
//! A layout is a tree of splits and tab stacks whose leaves are panels. It is stored as
//! editable TOML, and the *only* arithmetic that matters is `split`, which turns a rect and
//! a weight list into child rects -- cell-granular, remainder to the last child. Every
//! panel gets its rect from there, so a layout change cannot change what a panel draws.

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelKind {
    Tree,
    Table,
    Albums,
    NowPlaying,
    Properties,
    Console,
}

impl PanelKind {
    pub const ALL: [PanelKind; 6] = [
        PanelKind::Tree,
        PanelKind::Table,
        PanelKind::Albums,
        PanelKind::NowPlaying,
        PanelKind::Properties,
        PanelKind::Console,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PanelKind::Tree => "library",
            PanelKind::Table => "tracks",
            PanelKind::Albums => "albums",
            PanelKind::NowPlaying => "now playing",
            PanelKind::Properties => "properties",
            PanelKind::Console => "console",
        }
    }

    pub fn next(self) -> Self {
        let all = Self::ALL;
        let index = all.iter().position(|k| *k == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Node {
    Split {
        axis: Axis,
        weights: Vec<u16>,
        children: Vec<Node>,
    },
    Tabs {
        active: usize,
        children: Vec<Node>,
    },
    Panel {
        panel: PanelKind,
    },
}

impl Node {
    pub fn panel(panel: PanelKind) -> Self {
        Node::Panel { panel }
    }

    /// The layout a bare `fbp` starts with: library tree, a table/albums tab stack, and the
    /// now-playing/console pair -- fooyin's shape.
    pub fn default_layout() -> Self {
        Node::Split {
            axis: Axis::Horizontal,
            weights: vec![26, 44, 30],
            children: vec![
                Node::panel(PanelKind::Tree),
                Node::Tabs {
                    active: 0,
                    children: vec![
                        Node::panel(PanelKind::Table),
                        Node::panel(PanelKind::Albums),
                    ],
                },
                Node::Split {
                    axis: Axis::Vertical,
                    weights: vec![62, 38],
                    children: vec![
                        Node::Tabs {
                            active: 0,
                            children: vec![
                                Node::panel(PanelKind::NowPlaying),
                                Node::panel(PanelKind::Properties),
                            ],
                        },
                        Node::panel(PanelKind::Console),
                    ],
                },
            ],
        }
    }

    /// Every panel this layout currently shows, in draw order. A tab stack contributes only
    /// its active child, which is also the only one the focus can visit.
    pub fn leaves(&self) -> Vec<PanelKind> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PanelKind>) {
        match self {
            Node::Panel { panel } => out.push(*panel),
            Node::Tabs { active, children } => {
                if let Some(child) = children.get(*active) {
                    child.collect(out);
                }
            }
            Node::Split { children, .. } => {
                for child in children {
                    child.collect(out);
                }
            }
        }
    }

    /// Paths (child indices) to each visible leaf, in `leaves()` order.
    pub fn leaf_paths(&self) -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        self.collect_paths(&mut Vec::new(), &mut out);
        out
    }

    fn collect_paths(&self, prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        match self {
            Node::Panel { .. } => out.push(prefix.clone()),
            Node::Tabs { active, children } => {
                if children.get(*active).is_some() {
                    prefix.push(*active);
                    children[*active].collect_paths(prefix, out);
                    prefix.pop();
                }
            }
            Node::Split { children, .. } => {
                for (index, child) in children.iter().enumerate() {
                    prefix.push(index);
                    child.collect_paths(prefix, out);
                    prefix.pop();
                }
            }
        }
    }

    /// The node at a leaf path. Used by the tests today; the pixel overlay and the mouse
    /// hit-test in P3 resolve the same paths.
    #[allow(dead_code)]
    pub fn node_at(&self, path: &[usize]) -> Option<&Node> {
        let mut node = self;
        for index in path {
            node = match node {
                Node::Split { children, .. } | Node::Tabs { children, .. } => {
                    children.get(*index)?
                }
                Node::Panel { .. } => return None,
            };
        }
        Some(node)
    }

    fn node_at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        let mut node = self;
        for index in path {
            node = match node {
                Node::Split { children, .. } | Node::Tabs { children, .. } => {
                    children.get_mut(*index)?
                }
                Node::Panel { .. } => return None,
            };
        }
        Some(node)
    }

    /// Split the panel at `path` in two: the panel itself, plus a fresh neighbour.
    pub fn split_at(&mut self, path: &[usize], axis: Axis, new_panel: PanelKind) -> bool {
        let Some(node) = self.node_at_mut(path) else {
            return false;
        };
        let panel = match node {
            Node::Panel { panel } => *panel,
            _ => panel_of(node),
        };
        let replacement = Node::Split {
            axis,
            weights: vec![1, 1],
            children: vec![Node::panel(panel), Node::panel(new_panel)],
        };
        *node = replacement;
        true
    }

    /// Close the panel at `path`, folding a now-single-child split or tab stack away.
    pub fn close_at(&mut self, path: &[usize]) -> bool {
        if path.is_empty() {
            return false;
        }
        let (parent_path, index) = path.split_at(path.len() - 1);
        let index = index[0];
        let Some(parent) = self.node_at_mut(parent_path) else {
            return false;
        };
        let Some(children) = (match parent {
            Node::Split { children, .. } | Node::Tabs { children, .. } => Some(children),
            Node::Panel { .. } => None,
        }) else {
            return false;
        };
        if children.len() <= 1 {
            return false;
        }
        children.remove(index);
        if children.len() == 1 {
            let only = children.remove(0);
            *parent = only;
        }
        true
    }

    /// Nudge the boundary next to the panel at `path` by `delta` cells of weight.
    pub fn resize_at(&mut self, path: &[usize], delta: i16) -> bool {
        if path.is_empty() {
            return false;
        }
        let (parent_path, index) = path.split_at(path.len() - 1);
        let index = index[0];
        let Some(parent) = self.node_at_mut(parent_path) else {
            return false;
        };
        let Node::Split {
            weights, children, ..
        } = parent
        else {
            return false;
        };
        if weights.len() != children.len() || weights.len() < 2 {
            return false;
        }
        // Move weight between the focused child and its neighbour, never below 1.
        let neighbour = if index + 1 < weights.len() {
            index + 1
        } else {
            index - 1
        };
        if weights[index] == 0 || weights[neighbour] == 0 {
            return false;
        }
        let grow = (weights[neighbour] as i16) - 1;
        let shrink = -((weights[index] as i16) - 1);
        let moved = delta.clamp(shrink, grow);
        if moved == 0 {
            return false;
        }
        if moved > 0 {
            weights[index] += moved as u16;
            weights[neighbour] -= moved as u16;
        } else {
            let moved = (-moved) as u16;
            weights[index] -= moved;
            weights[neighbour] += moved;
        }
        true
    }
}

fn panel_of(node: &Node) -> PanelKind {
    match node {
        Node::Panel { panel } => *panel,
        Node::Tabs { children, active } => children
            .get(*active)
            .map(panel_of)
            .unwrap_or(PanelKind::Table),
        Node::Split { children, .. } => children.first().map(panel_of).unwrap_or(PanelKind::Table),
    }
}

/// Turn a rect and a weight list into child rects: cell-granular, the last child takes the
/// remainder so the children always tile the parent exactly.
pub fn split(rect: Rect, axis: Axis, weights: &[u16]) -> Vec<Rect> {
    if weights.is_empty() {
        return Vec::new();
    }
    let length = match axis {
        Axis::Horizontal => rect.width as u32,
        Axis::Vertical => rect.height as u32,
    };
    let total: u32 = weights
        .iter()
        .map(|w| (*w).max(1) as u32)
        .sum::<u32>()
        .max(1);
    let mut out = Vec::with_capacity(weights.len());
    let mut used = 0u32;

    for (index, weight) in weights.iter().enumerate() {
        let remaining = length.saturating_sub(used);
        let size = if index + 1 == weights.len() {
            remaining
        } else {
            ((length * (*weight).max(1) as u32) / total).clamp(1, remaining.max(1))
        };
        let size = size as u16;
        out.push(match axis {
            Axis::Horizontal => Rect {
                x: rect.x + used as u16,
                y: rect.y,
                width: size,
                height: rect.height,
            },
            Axis::Vertical => Rect {
                x: rect.x,
                y: rect.y + used as u16,
                width: rect.width,
                height: size,
            },
        });
        used += size as u32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    #[test]
    fn split_tiles_the_parent_exactly() {
        let parts = split(rect(10, 5), Axis::Horizontal, &[1, 1, 1]);
        assert_eq!(
            parts.iter().map(|r| r.width).collect::<Vec<_>>(),
            vec![3, 3, 4]
        );
        assert_eq!(parts[2].x, 6);
        let total: u16 = parts.iter().map(|r| r.width).sum();
        assert_eq!(total, 10, "children must tile the parent");
    }

    #[test]
    fn split_respects_weights_for_uneven_regions() {
        let parts = split(rect(26, 10), Axis::Vertical, &[62, 38]);
        assert_eq!(parts[0].height + parts[1].height, 10);
        assert!(parts[0].height > parts[1].height);
    }

    #[test]
    fn a_tab_stack_contributes_only_its_active_child() {
        let layout = Node::default_layout();
        assert_eq!(
            layout.leaves(),
            vec![
                PanelKind::Tree,
                PanelKind::Table,
                PanelKind::NowPlaying,
                PanelKind::Console
            ]
        );
        assert_eq!(layout.leaf_paths().len(), 4);
    }

    #[test]
    fn the_default_layout_round_trips_through_toml() {
        let layout = Node::default_layout();
        let text = toml::to_string(&File {
            node: layout.clone(),
        })
        .expect("serialize");
        let back: File = toml::from_str(&text).expect("parse");
        assert_eq!(back.node, layout);
    }

    #[test]
    fn split_close_and_resize_keep_the_tree_well_formed() {
        let mut layout = Node::default_layout();
        let paths = layout.leaf_paths();
        let table = paths[1].clone();

        assert!(layout.split_at(&table, Axis::Vertical, PanelKind::Console));
        assert_eq!(layout.leaves().len(), 5);

        // The new neighbour is the second child of the split we just created.
        let mut second = table.clone();
        second.push(1);
        assert!(layout.close_at(&second), "close the panel we just made");
        assert_eq!(layout.leaves().len(), 4);

        // Resizing moves weight, never below one cell of weight.
        let path = layout.leaf_paths()[0].clone();
        assert!(layout.resize_at(&path, 4));
        assert!(layout.resize_at(&path, -100));
        if let Some(Node::Split { weights, .. }) = layout.node_at(&[]) {
            assert!(weights.iter().all(|w| *w >= 1));
        }
    }

    #[test]
    fn closing_the_last_panel_is_refused() {
        let mut layout = Node::Panel {
            panel: PanelKind::Table,
        };
        assert!(!layout.close_at(&[]));
        assert_eq!(layout.leaves(), vec![PanelKind::Table]);
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct File {
    pub node: Node,
}
