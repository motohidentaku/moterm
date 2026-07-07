//! タブ内ペインの二分木。分割・フォーカス移動・クローズを純ロジックとして実装（テスト可能）。
//! 葉が実ペイン（SSH シェル）を指す。矩形割当も純関数。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// 左右分割（縦の境界線）
    Horizontal,
    /// 上下分割（横の境界線）
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
}

/// ペイン木。葉は pane_id（u64）を持つ。
#[derive(Debug, Clone)]
pub enum PaneNode {
    Leaf {
        pane_id: u64,
    },
    Split {
        dir: SplitDir,
        /// 分割比（first の割合 0.0..1.0）
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

/// 画面上のセル矩形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl PaneNode {
    pub fn leaf(pane_id: u64) -> Self {
        PaneNode::Leaf { pane_id }
    }

    /// フォーカス中の葉を分割し、新しい葉に new_id を割り当てる。成功なら true。
    pub fn split(&mut self, focus: u64, dir: SplitDir, new_id: u64) -> bool {
        match self {
            PaneNode::Leaf { pane_id } if *pane_id == focus => {
                let old = *pane_id;
                *self = PaneNode::Split {
                    dir,
                    ratio: 0.5,
                    first: Box::new(PaneNode::leaf(old)),
                    second: Box::new(PaneNode::leaf(new_id)),
                };
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { first, second, .. } => {
                first.split(focus, dir, new_id) || second.split(focus, dir, new_id)
            }
        }
    }

    /// 指定 pane_id の葉を閉じる。木が空になる場合は None を返す（タブごと閉じる合図）。
    /// 返り値: Some(残った木)。
    pub fn close(self, target: u64) -> Option<PaneNode> {
        match self {
            PaneNode::Leaf { pane_id } => {
                if pane_id == target {
                    None
                } else {
                    Some(PaneNode::Leaf { pane_id })
                }
            }
            PaneNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let f = first.close(target);
                let s = second.close(target);
                match (f, s) {
                    (Some(f), Some(s)) => Some(PaneNode::Split {
                        dir,
                        ratio,
                        first: Box::new(f),
                        second: Box::new(s),
                    }),
                    // 片方が消えたら、残った方が昇格する
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                }
            }
        }
    }

    /// 全葉の pane_id を列挙（出現順）。
    pub fn leaves(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<u64>) {
        match self {
            PaneNode::Leaf { pane_id } => out.push(*pane_id),
            PaneNode::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        self.leaves().contains(&id)
    }

    /// 木全体を rect に割り当て、各葉の矩形を返す。
    pub fn layout(&self, rect: Rect) -> Vec<(u64, Rect)> {
        let mut out = Vec::new();
        self.layout_into(rect, &mut out);
        out
    }

    fn layout_into(&self, rect: Rect, out: &mut Vec<(u64, Rect)>) {
        match self {
            PaneNode::Leaf { pane_id } => out.push((*pane_id, rect)),
            PaneNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_rect(rect, *dir, *ratio);
                first.layout_into(a, out);
                second.layout_into(b, out);
            }
        }
    }

    /// フォーカス移動: 現在の葉の矩形中心から、方向にある最も近い葉を選ぶ。
    pub fn focus_move(&self, rect: Rect, current: u64, mv: Move) -> Option<u64> {
        let layout = self.layout(rect);
        let cur = layout.iter().find(|(id, _)| *id == current)?;
        let (cx, cy) = center(cur.1);
        let mut best: Option<(u64, i64)> = None;
        for (id, r) in &layout {
            if *id == current {
                continue;
            }
            let (ox, oy) = center(*r);
            let ok = match mv {
                Move::Left => ox < cx,
                Move::Right => ox > cx,
                Move::Up => oy < cy,
                Move::Down => oy > cy,
            };
            if !ok {
                continue;
            }
            // 主軸の距離を優先、副軸のズレをペナルティに
            let (main, cross) = match mv {
                Move::Left | Move::Right => ((ox - cx).abs(), (oy - cy).abs()),
                Move::Up | Move::Down => ((oy - cy).abs(), (ox - cx).abs()),
            };
            let score = main as i64 + cross as i64 * 3;
            if best.map(|(_, s)| score < s).unwrap_or(true) {
                best = Some((*id, score));
            }
        }
        best.map(|(id, _)| id)
    }

    /// 座標 (col,row) を含む葉を返す（クリックでのフォーカス切替）。
    pub fn pane_at(&self, rect: Rect, col: u16, row: u16) -> Option<u64> {
        self.layout(rect).into_iter().find_map(|(id, r)| {
            if col >= r.x && col < r.x + r.w && row >= r.y && row < r.y + r.h {
                Some(id)
            } else {
                None
            }
        })
    }
}

fn split_rect(rect: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect) {
    match dir {
        SplitDir::Horizontal => {
            // 左右。1 セルを境界に使う。
            let total = rect.w.saturating_sub(1);
            let first_w = ((total as f32) * ratio).round() as u16;
            let a = Rect {
                x: rect.x,
                y: rect.y,
                w: first_w,
                h: rect.h,
            };
            let b = Rect {
                x: rect.x + first_w + 1,
                y: rect.y,
                w: total.saturating_sub(first_w),
                h: rect.h,
            };
            (a, b)
        }
        SplitDir::Vertical => {
            let total = rect.h.saturating_sub(1);
            let first_h = ((total as f32) * ratio).round() as u16;
            let a = Rect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: first_h,
            };
            let b = Rect {
                x: rect.x,
                y: rect.y + first_h + 1,
                w: rect.w,
                h: total.saturating_sub(first_h),
            };
            (a, b)
        }
    }
}

fn center(r: Rect) -> (i32, i32) {
    (r.x as i32 + r.w as i32 / 2, r.y as i32 + r.h as i32 / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_leaves() {
        let mut t = PaneNode::leaf(1);
        assert!(t.split(1, SplitDir::Horizontal, 2));
        assert_eq!(t.leaves(), vec![1, 2]);
        assert!(t.split(2, SplitDir::Vertical, 3));
        assert_eq!(t.leaves(), vec![1, 2, 3]);
        // 存在しない focus では失敗
        assert!(!t.split(99, SplitDir::Vertical, 4));
    }

    #[test]
    fn close_promotes_sibling() {
        let mut t = PaneNode::leaf(1);
        t.split(1, SplitDir::Horizontal, 2);
        t.split(2, SplitDir::Vertical, 3);
        // 3 を閉じると 2 が昇格
        let t = t.close(3).unwrap();
        assert_eq!(t.leaves(), vec![1, 2]);
        // 全部閉じると None
        let t = t.close(1).unwrap();
        assert_eq!(t.leaves(), vec![2]);
        assert!(t.close(2).is_none());
    }

    #[test]
    fn layout_covers_rect() {
        let mut t = PaneNode::leaf(1);
        t.split(1, SplitDir::Horizontal, 2);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 81,
            h: 24,
        };
        let l = t.layout(rect);
        assert_eq!(l.len(), 2);
        // 左 40, 境界 1, 右 40
        assert_eq!(l[0].1.w, 40);
        assert_eq!(l[1].1.x, 41);
        assert_eq!(l[1].1.w, 40);
    }

    #[test]
    fn focus_move_directional() {
        let mut t = PaneNode::leaf(1);
        t.split(1, SplitDir::Horizontal, 2); // 1 左, 2 右
        let rect = Rect {
            x: 0,
            y: 0,
            w: 81,
            h: 24,
        };
        assert_eq!(t.focus_move(rect, 1, Move::Right), Some(2));
        assert_eq!(t.focus_move(rect, 2, Move::Left), Some(1));
        assert_eq!(t.focus_move(rect, 1, Move::Left), None);
    }

    #[test]
    fn pane_at_hit() {
        let mut t = PaneNode::leaf(1);
        t.split(1, SplitDir::Horizontal, 2);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 81,
            h: 24,
        };
        assert_eq!(t.pane_at(rect, 5, 5), Some(1));
        assert_eq!(t.pane_at(rect, 60, 5), Some(2));
    }
}
