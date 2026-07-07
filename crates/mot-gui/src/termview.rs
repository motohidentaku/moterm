//! 端末スクリーンを1ドローでフレームバッファのセル矩形へ描画する。
//! グリフはフォントキャッシュ（atlas 相当の HashMap）から取得。全角は2セル。

use crate::font::FontManager;
use crate::render::Framebuffer;
use crate::selection::Selection;
use crate::theme::{blend, Theme};
use mot_term::{Attrs, Screen};

/// 検索マッチのハイライト背景（0RGB）。カレントは強調。
const SEARCH_MATCH: u32 = 0x0080_6000; // 暗い黄
const SEARCH_CURRENT: u32 = 0x00C0_8000; // 橙

/// 端末描画のピクセル指標。
pub struct CellMetrics {
    pub cw: i32,
    pub ch: i32,
    pub px: f32,
    pub baseline: i32,
}

impl CellMetrics {
    pub fn new(font: &mut FontManager, px: f32) -> Self {
        CellMetrics {
            cw: font.cell_width(px),
            ch: font.cell_height(px),
            px,
            baseline: font.baseline(px),
        }
    }
}

/// 検索ハイライト: マッチ範囲 (abs_row, c0, c1) の一覧と、カレントマッチのインデックス。
#[derive(Clone, Copy)]
pub struct SearchHl<'a> {
    pub matches: &'a [(usize, usize, usize)],
    pub current: usize,
}

/// px 原点 (ox,oy) に、scroll_offset 行だけ遡ったビューを描画する。
/// search があればマッチ列範囲をハイライト（カレントマッチは別色）。
#[allow(clippy::too_many_arguments)]
pub fn draw_screen(
    fb: &mut Framebuffer,
    font: &mut FontManager,
    theme: &Theme,
    screen: &Screen,
    m: &CellMetrics,
    ox: i32,
    oy: i32,
    scroll_offset: usize,
    selection: Option<&Selection>,
    search: Option<SearchHl>,
) {
    let rows = screen.rows();
    let cols = screen.cols();
    let sb = screen.scrollback_len();
    // 絶対行の起点（このビュー最上段が絶対何行目か）
    let top_abs = (sb + rows).saturating_sub(rows + scroll_offset);

    for vr in 0..rows {
        let line = screen.view_line(vr, scroll_offset);
        let abs_row = top_abs + vr;
        let py = oy + vr as i32 * m.ch;
        let mut col = 0usize;
        while col < cols && col < line.len() {
            let cell = &line[col];
            if cell.attrs.contains(Attrs::WIDE_TRAILER) {
                col += 1;
                continue;
            }
            let wide = cell.attrs.contains(Attrs::WIDE);
            let cell_px_w = if wide { m.cw * 2 } else { m.cw };
            let px = ox + col as i32 * m.cw;

            // 色（REVERSE を反映）
            let mut fg = theme.resolve_fg(cell.fg);
            let mut bg = theme.resolve_bg(cell.bg);
            if cell.attrs.contains(Attrs::REVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.attrs.contains(Attrs::DIM) {
                fg = blend(bg, fg, 0.6);
            }
            // 選択ハイライト
            let selected = selection.map(|s| s.contains(abs_row, col)).unwrap_or(false);
            if selected {
                bg = theme.selection;
            }
            // 検索ハイライト（マッチ範囲。カレントマッチは強調色）
            if let Some(hl) = &search {
                for (k, (mr, c0, c1)) in hl.matches.iter().enumerate() {
                    if *mr == abs_row && col >= *c0 && col < *c1 {
                        bg = if k == hl.current {
                            SEARCH_CURRENT
                        } else {
                            SEARCH_MATCH
                        };
                        break;
                    }
                }
            }

            // 背景
            if bg != theme.bg || selected {
                fb.fill_rect(px, py, cell_px_w, m.ch, bg);
            }

            // グリフ
            if cell.ch != ' ' && !cell.attrs.contains(Attrs::HIDDEN) {
                let bold = cell.attrs.contains(Attrs::BOLD);
                let g = font.glyph(cell.ch, m.px, bold);
                let gx = px + g.left;
                let gy = py + m.baseline - g.top;
                fb.blit_coverage(gx, gy, &g.bitmap, fg);
            }
            // 下線・取消線
            if cell.attrs.contains(Attrs::UNDERLINE) {
                fb.fill_rect(px, py + m.ch - 2, cell_px_w, 1, fg);
            }
            if cell.attrs.contains(Attrs::STRIKETHROUGH) {
                fb.fill_rect(px, py + m.ch / 2, cell_px_w, 1, fg);
            }

            col += if wide { 2 } else { 1 };
        }
    }

    // カーソル（スクロールしていない & 可視のとき）
    if scroll_offset == 0 && screen.cursor_visible {
        let cy = oy + screen.cursor.row as i32 * m.ch;
        let cx = ox + screen.cursor.col as i32 * m.cw;
        fb.fill_rect(cx, cy, m.cw, m.ch, theme.cursor);
        // カーソル下の文字を反転色で重ねる
        if let Some(line) = screen_line(screen, screen.cursor.row) {
            if let Some(cell) = line.get(screen.cursor.col) {
                if cell.ch != ' ' {
                    let g = font.glyph(cell.ch, m.px, false);
                    fb.blit_coverage(cx + g.left, cy + m.baseline - g.top, &g.bitmap, theme.bg);
                }
            }
        }
    }
}

fn screen_line(screen: &Screen, row: usize) -> Option<&Vec<mot_term::Cell>> {
    if row < screen.rows() {
        Some(screen.view_line(row, 0))
    } else {
        None
    }
}

/// 端末ピクセル領域 (w,h) から収まるセル数 (cols,rows) を算出。
pub fn cells_for(m: &CellMetrics, w: i32, h: i32) -> (u16, u16) {
    let cols = (w / m.cw).max(1) as u16;
    let rows = (h / m.ch).max(1) as u16;
    (cols, rows)
}

/// スクロールバーのサム位置とサイズ（px）。(top, height)。全体が見えていれば None。
pub fn scrollbar_thumb(
    track_h: i32,
    total_lines: usize,
    view_lines: usize,
    offset: usize,
) -> Option<(i32, i32)> {
    if total_lines <= view_lines {
        return None;
    }
    let ratio = view_lines as f32 / total_lines as f32;
    let thumb_h = (track_h as f32 * ratio).max(12.0) as i32;
    // offset=0 は最下部
    let scroll_pos = total_lines - view_lines - offset.min(total_lines - view_lines);
    let max_scroll = (total_lines - view_lines) as f32;
    let t = scroll_pos as f32 / max_scroll;
    let top = (t * (track_h - thumb_h) as f32) as i32;
    Some((top, thumb_h))
}

/// `scrollbar_thumb` の逆写像。サム上端 y（px、範囲外は内部でクランプ）から
/// scrollback オフセット（0=最下部/ライブ）を求める。全体が収まる場合は 0。
/// サム上端 0 → 最古（offset=max）、サム下端 → ライブ（offset=0）。
pub fn scroll_from_thumb_top(
    track_h: i32,
    total_lines: usize,
    view_lines: usize,
    thumb_top: i32,
) -> usize {
    if total_lines <= view_lines {
        return 0;
    }
    let ratio = view_lines as f32 / total_lines as f32;
    let thumb_h = (track_h as f32 * ratio).max(12.0) as i32;
    let denom = (track_h - thumb_h).max(1);
    let t = thumb_top.clamp(0, denom) as f32 / denom as f32;
    let max_scroll = total_lines - view_lines;
    ((1.0 - t) * max_scroll as f32).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_computation() {
        let m = CellMetrics {
            cw: 8,
            ch: 16,
            px: 14.0,
            baseline: 12,
        };
        assert_eq!(cells_for(&m, 800, 480), (100, 30));
        assert_eq!(cells_for(&m, 4, 4), (1, 1)); // 最低1
    }

    #[test]
    fn scrollbar_hidden_when_fits() {
        assert!(scrollbar_thumb(200, 20, 24, 0).is_none());
    }

    #[test]
    fn scrollbar_thumb_bounds() {
        let (top, h) = scrollbar_thumb(200, 100, 20, 0).unwrap();
        assert!(h >= 12);
        // 最下部（offset 0）はサムが下端付近
        assert!(top + h <= 200);
        let (top_up, _) = scrollbar_thumb(200, 100, 20, 80).unwrap();
        assert!(top_up < top); // 遡るとサムは上へ
    }

    #[test]
    fn thumb_top_roundtrips_to_offset() {
        // scrollbar_thumb(offset) → top、その top を scroll_from_thumb_top へ戻すと
        // 元の offset に一致する（サム位置とスクロール量が可逆）。
        let (track_h, total, view) = (200, 100, 20);
        for offset in [0usize, 1, 20, 40, 79, 80] {
            let (top, _h) = scrollbar_thumb(track_h, total, view, offset).unwrap();
            let back = scroll_from_thumb_top(track_h, total, view, top);
            assert!(
                (back as i32 - offset as i32).abs() <= 1,
                "offset={offset} top={top} back={back}"
            );
        }
    }

    #[test]
    fn thumb_top_extremes() {
        let (track_h, total, view) = (200, 100, 20);
        // 上端 → 最古（max=total-view=80）、下端 → ライブ(0)
        assert_eq!(scroll_from_thumb_top(track_h, total, view, 0), 80);
        assert_eq!(scroll_from_thumb_top(track_h, total, view, 100_000), 0);
        // 収まっていればスクロール不可
        assert_eq!(scroll_from_thumb_top(track_h, 20, view, 50), 0);
    }
}
