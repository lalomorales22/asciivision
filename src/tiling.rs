use ratatui::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelKind {
    Transcript,
    Games,
    Tiles,
    Video,
    Webcam,
    Telemetry,
    OpsDeck,
    Effects3D,
    Analytics,
    VideoChatFeeds,
    VideoChatMessages,
    VideoChatUsers,
    SystemMonitor,
}

impl PanelKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Transcript => "TRANSCRIPT",
            Self::Games => "GAMES",
            Self::Tiles => "TILES",
            Self::Video => "VIDEO BUS",
            Self::Webcam => "WEBCAM",
            Self::Telemetry => "TELEMETRY",
            Self::OpsDeck => "OPS DECK",
            Self::Effects3D => "3D EFFECTS",
            Self::Analytics => "ANALYTICS",
            Self::VideoChatFeeds => "VC FEEDS",
            Self::VideoChatMessages => "VC CHAT",
            Self::VideoChatUsers => "VC USERS",
            Self::SystemMonitor => "SYS MONITOR",
        }
    }

    pub fn cycle_next(self) -> Self {
        match self {
            Self::Transcript => Self::Games,
            Self::Games => Self::Tiles,
            Self::Tiles => Self::Video,
            Self::Video => Self::Webcam,
            Self::Webcam => Self::Telemetry,
            Self::Telemetry => Self::OpsDeck,
            Self::OpsDeck => Self::Effects3D,
            Self::Effects3D => Self::Analytics,
            Self::Analytics => Self::SystemMonitor,
            Self::SystemMonitor => Self::VideoChatFeeds,
            Self::VideoChatFeeds => Self::VideoChatMessages,
            Self::VideoChatMessages => Self::VideoChatUsers,
            Self::VideoChatUsers => Self::Transcript,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone)]
pub enum TileNode {
    Leaf {
        panel: PanelKind,
        id: usize,
    },
    Split {
        direction: SplitDir,
        ratio: f32,
        first: Box<TileNode>,
        second: Box<TileNode>,
    },
}

impl TileNode {
    fn leaf(panel: PanelKind, id: usize) -> Self {
        Self::Leaf { panel, id }
    }

    fn hsplit(ratio: f32, first: TileNode, second: TileNode) -> Self {
        Self::Split {
            direction: SplitDir::Horizontal,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn vsplit(ratio: f32, first: TileNode, second: TileNode) -> Self {
        Self::Split {
            direction: SplitDir::Vertical,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    pub fn collect_leaves(&self) -> Vec<(usize, PanelKind)> {
        let mut result = Vec::new();
        self.collect_leaves_inner(&mut result);
        result
    }

    fn collect_leaves_inner(&self, out: &mut Vec<(usize, PanelKind)>) {
        match self {
            Self::Leaf { panel, id } => out.push((*id, *panel)),
            Self::Split { first, second, .. } => {
                first.collect_leaves_inner(out);
                second.collect_leaves_inner(out);
            }
        }
    }

    pub fn compute_rects(&self, area: Rect) -> Vec<(usize, PanelKind, Rect)> {
        let mut result = Vec::new();
        self.compute_rects_inner(area, &mut result);
        result
    }

    const MIN_W: u16 = 10;
    const MIN_H: u16 = 5;

    fn compute_rects_inner(&self, area: Rect, out: &mut Vec<(usize, PanelKind, Rect)>) {
        if area.width < Self::MIN_W || area.height < Self::MIN_H {
            return;
        }
        match self {
            Self::Leaf { panel, id } => {
                out.push((*id, *panel, area));
            }
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (a1, a2) = split_rect(area, *direction, *ratio);
                let a1_ok = a1.width >= Self::MIN_W && a1.height >= Self::MIN_H;
                let a2_ok = a2.width >= Self::MIN_W && a2.height >= Self::MIN_H;
                match (a1_ok, a2_ok) {
                    (true, true) => {
                        first.compute_rects_inner(a1, out);
                        second.compute_rects_inner(a2, out);
                    }
                    (true, false) => {
                        // give all space to first child
                        first.compute_rects_inner(area, out);
                    }
                    (false, true) => {
                        // give all space to second child
                        second.compute_rects_inner(area, out);
                    }
                    (false, false) => {
                        // area too small for split, give it all to first
                        first.compute_rects_inner(area, out);
                    }
                }
            }
        }
    }

    fn find_leaf_mut(&mut self, id: usize) -> Option<&mut PanelKind> {
        match self {
            Self::Leaf {
                panel,
                id: leaf_id,
            } => {
                if *leaf_id == id {
                    Some(panel)
                } else {
                    None
                }
            }
            Self::Split { first, second, .. } => first
                .find_leaf_mut(id)
                .or_else(|| second.find_leaf_mut(id)),
        }
    }

    fn find_split_containing(&mut self, child_id: usize) -> Option<&mut f32> {
        match self {
            Self::Leaf { .. } => None,
            Self::Split {
                ratio,
                first,
                second,
                ..
            } => {
                if first.contains_leaf(child_id) || second.contains_leaf(child_id) {
                    Some(ratio)
                } else {
                    first
                        .find_split_containing(child_id)
                        .or_else(|| second.find_split_containing(child_id))
                }
            }
        }
    }

    fn contains_leaf(&self, id: usize) -> bool {
        match self {
            Self::Leaf { id: leaf_id, .. } => *leaf_id == id,
            Self::Split { first, second, .. } => {
                first.contains_leaf(id) || second.contains_leaf(id)
            }
        }
    }
}

fn split_rect(area: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect) {
    let ratio = ratio.clamp(0.1, 0.9);
    match dir {
        SplitDir::Horizontal => {
            let left_w = (area.width as f32 * ratio).round() as u16;
            let right_w = area.width.saturating_sub(left_w);
            (
                Rect::new(area.x, area.y, left_w, area.height),
                Rect::new(area.x + left_w, area.y, right_w, area.height),
            )
        }
        SplitDir::Vertical => {
            let top_h = (area.height as f32 * ratio).round() as u16;
            let bot_h = area.height.saturating_sub(top_h);
            (
                Rect::new(area.x, area.y, area.width, top_h),
                Rect::new(area.x, area.y + top_h, area.width, bot_h),
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutPreset {
    Default,
    DualPane,
    TripleColumn,
    Quad,
    WebcamFocus,
    FullFocus,
}

impl LayoutPreset {
    pub fn cycle(self) -> Self {
        match self {
            Self::Default => Self::DualPane,
            Self::DualPane => Self::TripleColumn,
            Self::TripleColumn => Self::Quad,
            Self::Quad => Self::WebcamFocus,
            Self::WebcamFocus => Self::FullFocus,
            Self::FullFocus => Self::Default,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Default => "DEFAULT",
            Self::DualPane => "DUAL PANE",
            Self::TripleColumn => "TRIPLE COL",
            Self::Quad => "QUAD",
            Self::WebcamFocus => "WEBCAM FOCUS",
            Self::FullFocus => "FULL FOCUS",
        }
    }
}

pub struct TilingManager {
    pub tree: TileNode,
    pub focused: usize,
    pub preset: LayoutPreset,
    next_id: usize,
}

impl TilingManager {
    pub fn new() -> Self {
        let mut mgr = Self {
            tree: TileNode::leaf(PanelKind::Transcript, 0),
            focused: 0,
            preset: LayoutPreset::Default,
            next_id: 0,
        };
        mgr.apply_preset(LayoutPreset::Default);
        mgr
    }

    pub fn apply_preset(&mut self, preset: LayoutPreset) {
        self.preset = preset;
        self.next_id = 0;
        self.tree = match preset {
            LayoutPreset::Default => {
                // [Transcript 55% | V[ Video/Effects3D 35% / Webcam 20% / V[ Telemetry / SysMon ] 22% / Ops 23% ]]
                TileNode::hsplit(
                    0.55,
                    self.make_leaf(PanelKind::Transcript),
                    TileNode::vsplit(
                        0.35,
                        TileNode::hsplit(
                            0.55,
                            self.make_leaf(PanelKind::Video),
                            self.make_leaf(PanelKind::Effects3D),
                        ),
                        TileNode::vsplit(
                            0.30,
                            self.make_leaf(PanelKind::Webcam),
                            TileNode::vsplit(
                                0.50,
                                TileNode::hsplit(
                                    0.50,
                                    self.make_leaf(PanelKind::Telemetry),
                                    self.make_leaf(PanelKind::SystemMonitor),
                                ),
                                self.make_leaf(PanelKind::OpsDeck),
                            ),
                        ),
                    ),
                )
            }
            LayoutPreset::DualPane => TileNode::hsplit(
                0.50,
                self.make_leaf(PanelKind::Transcript),
                TileNode::vsplit(
                    0.34,
                    self.make_leaf(PanelKind::Effects3D),
                    TileNode::vsplit(
                        0.50,
                        self.make_leaf(PanelKind::Webcam),
                        self.make_leaf(PanelKind::SystemMonitor),
                    ),
                ),
            ),
            LayoutPreset::TripleColumn => TileNode::hsplit(
                0.38,
                self.make_leaf(PanelKind::Transcript),
                TileNode::hsplit(
                    0.50,
                    TileNode::vsplit(
                        0.50,
                        self.make_leaf(PanelKind::Video),
                        self.make_leaf(PanelKind::Effects3D),
                    ),
                    TileNode::vsplit(
                        0.40,
                        self.make_leaf(PanelKind::Webcam),
                        TileNode::vsplit(
                            0.50,
                            self.make_leaf(PanelKind::SystemMonitor),
                            self.make_leaf(PanelKind::OpsDeck),
                        ),
                    ),
                ),
            ),
            LayoutPreset::Quad => TileNode::vsplit(
                0.50,
                TileNode::hsplit(
                    0.50,
                    self.make_leaf(PanelKind::Transcript),
                    self.make_leaf(PanelKind::Video),
                ),
                TileNode::hsplit(
                    0.34,
                    self.make_leaf(PanelKind::Effects3D),
                    TileNode::hsplit(
                        0.50,
                        self.make_leaf(PanelKind::Webcam),
                        self.make_leaf(PanelKind::SystemMonitor),
                    ),
                ),
            ),
            LayoutPreset::WebcamFocus => TileNode::hsplit(
                0.35,
                TileNode::vsplit(
                    0.60,
                    self.make_leaf(PanelKind::Transcript),
                    self.make_leaf(PanelKind::SystemMonitor),
                ),
                TileNode::vsplit(
                    0.55,
                    self.make_leaf(PanelKind::Webcam),
                    TileNode::vsplit(
                        0.50,
                        self.make_leaf(PanelKind::Effects3D),
                        TileNode::hsplit(
                            0.50,
                            self.make_leaf(PanelKind::Telemetry),
                            self.make_leaf(PanelKind::OpsDeck),
                        ),
                    ),
                ),
            ),
            LayoutPreset::FullFocus => self.make_leaf(PanelKind::Transcript),
        };
        self.focused = 0;
    }

    fn make_leaf(&mut self, panel: PanelKind) -> TileNode {
        let id = self.next_id;
        self.next_id += 1;
        TileNode::leaf(panel, id)
    }

    pub fn layout(&self, area: Rect) -> Vec<(usize, PanelKind, Rect)> {
        self.tree.compute_rects(area)
    }

    pub fn leaves(&self) -> Vec<(usize, PanelKind)> {
        self.tree.collect_leaves()
    }

    #[allow(dead_code)]
    pub fn leaf_count(&self) -> usize {
        self.tree.collect_leaves().len()
    }

    pub fn focused_panel(&self) -> Option<PanelKind> {
        self.tree
            .collect_leaves()
            .iter()
            .find(|(id, _)| *id == self.focused)
            .map(|(_, p)| *p)
    }

    #[allow(dead_code)]
    pub fn focus_next(&mut self) {
        let leaves = self.leaves();
        if leaves.is_empty() {
            return;
        }
        if let Some(pos) = leaves.iter().position(|(id, _)| *id == self.focused) {
            self.focused = leaves[(pos + 1) % leaves.len()].0;
        } else {
            self.focused = leaves[0].0;
        }
    }

    #[allow(dead_code)]
    pub fn focus_prev(&mut self) {
        let leaves = self.leaves();
        if leaves.is_empty() {
            return;
        }
        if let Some(pos) = leaves.iter().position(|(id, _)| *id == self.focused) {
            self.focused = leaves[(pos + leaves.len() - 1) % leaves.len()].0;
        } else {
            self.focused = leaves[0].0;
        }
    }

    pub fn focus_direction(&mut self, area: Rect, dx: i32, dy: i32) {
        let rects = self.layout(area);
        let current = match rects.iter().find(|(id, _, _)| *id == self.focused) {
            Some(r) => r,
            None => return,
        };
        let cx = current.2.x as i32 + current.2.width as i32 / 2;
        let cy = current.2.y as i32 + current.2.height as i32 / 2;

        let mut best: Option<(usize, i32)> = None;
        for &(id, _, rect) in &rects {
            if id == self.focused {
                continue;
            }
            let ox = rect.x as i32 + rect.width as i32 / 2;
            let oy = rect.y as i32 + rect.height as i32 / 2;
            let ddx = ox - cx;
            let ddy = oy - cy;

            let aligned = if dx != 0 {
                ddx.signum() == dx.signum()
            } else {
                ddy.signum() == dy.signum()
            };
            if !aligned {
                continue;
            }

            let dist = ddx.abs() + ddy.abs();
            if best.map_or(true, |(_, d)| dist < d) {
                best = Some((id, dist));
            }
        }

        if let Some((id, _)) = best {
            self.focused = id;
        }
    }

    pub fn swap_focused_with_direction(&mut self, area: Rect, dx: i32, dy: i32) {
        let rects = self.layout(area);
        let current = match rects.iter().find(|(id, _, _)| *id == self.focused) {
            Some(r) => r,
            None => return,
        };
        let cx = current.2.x as i32 + current.2.width as i32 / 2;
        let cy = current.2.y as i32 + current.2.height as i32 / 2;

        let mut best: Option<(usize, i32)> = None;
        for &(id, _, rect) in &rects {
            if id == self.focused {
                continue;
            }
            let ox = rect.x as i32 + rect.width as i32 / 2;
            let oy = rect.y as i32 + rect.height as i32 / 2;
            let ddx = ox - cx;
            let ddy = oy - cy;

            let aligned = if dx != 0 {
                ddx.signum() == dx.signum()
            } else {
                ddy.signum() == dy.signum()
            };
            if !aligned {
                continue;
            }
            let dist = ddx.abs() + ddy.abs();
            if best.map_or(true, |(_, d)| dist < d) {
                best = Some((id, dist));
            }
        }

        if let Some((target_id, _)) = best {
            let leaves = self.leaves();
            let panel_a = leaves
                .iter()
                .find(|(id, _)| *id == self.focused)
                .map(|(_, p)| *p);
            let panel_b = leaves
                .iter()
                .find(|(id, _)| *id == target_id)
                .map(|(_, p)| *p);

            if let (Some(pa), Some(pb)) = (panel_a, panel_b) {
                if let Some(leaf) = self.tree.find_leaf_mut(self.focused) {
                    *leaf = pb;
                }
                if let Some(leaf) = self.tree.find_leaf_mut(target_id) {
                    *leaf = pa;
                }
            }
        }
    }

    pub fn resize_focused(&mut self, delta: f32) {
        if let Some(ratio) = self.tree.find_split_containing(self.focused) {
            *ratio = (*ratio + delta).clamp(0.15, 0.85);
        }
    }

    pub fn cycle_focused_panel(&mut self) {
        if let Some(leaf) = self.tree.find_leaf_mut(self.focused) {
            *leaf = leaf.cycle_next();
        }
    }

    pub fn set_focused_panel(&mut self, panel: PanelKind) {
        if let Some(leaf) = self.tree.find_leaf_mut(self.focused) {
            *leaf = panel;
        }
    }
}
