//! A vertically scrollable block of rendered rows.
//!
//! `ScrollText` owns a scroll offset plus the geometry captured at the last
//! render, and the clamp/page arithmetic that keeps the offset in range. The
//! custom-command output pane, the Ask AI answer pane, and the keymap help
//! overlay each hold one and drive it through this interface instead of
//! re-deriving the same math at each call site.

/// A vertical scroll direction, shared by every scrollable surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerticalDirection {
    Down,
    Up,
}

impl VerticalDirection {
    pub(crate) fn shift(self, value: usize, amount: usize) -> usize {
        match self {
            Self::Down => value.saturating_add(amount),
            Self::Up => value.saturating_sub(amount),
        }
    }

    pub(crate) fn shift_clamped(self, value: usize, amount: usize, max: usize) -> usize {
        self.shift(value, amount).min(max)
    }
}

/// Scroll offset and last-rendered geometry for one scrollable pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ScrollText {
    offset: usize,
    rendered_row_count: usize,
    visible_height: usize,
}

impl ScrollText {
    /// First visible row, already clamped to the latest [`sync`](Self::sync).
    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    /// Records the geometry from the current render and clamps the offset to it.
    /// Call once per frame before reading [`offset`](Self::offset) or taking a
    /// [`visible`](Self::visible) slice.
    pub(crate) fn sync(&mut self, rendered_row_count: usize, visible_height: usize) {
        self.rendered_row_count = rendered_row_count;
        self.visible_height = visible_height;
        self.clamp();
    }

    /// Page step for `PageUp`/`PageDown`: the last rendered visible height.
    pub(crate) fn page(&self) -> usize {
        self.visible_height.max(1)
    }

    pub(crate) fn scroll_by(&mut self, direction: VerticalDirection, amount: usize) {
        self.offset = direction.shift(self.offset, amount);
        self.clamp();
    }

    pub(crate) fn scroll_to_top(&mut self) {
        self.offset = 0;
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        self.offset = usize::MAX;
        self.clamp();
    }

    /// The slice of `lines` visible at the current offset and height.
    pub(crate) fn visible<T>(&self, lines: Vec<T>) -> Vec<T> {
        lines
            .into_iter()
            .skip(self.offset)
            .take(self.visible_height)
            .collect()
    }

    fn clamp(&mut self) {
        self.offset = self
            .offset
            .min(self.rendered_row_count.saturating_sub(self.visible_height));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synced(offset: usize, rows: usize, height: usize) -> ScrollText {
        let mut scroll = ScrollText::default();
        scroll.sync(rows, height);
        scroll.scroll_by(VerticalDirection::Down, offset);
        scroll
    }

    #[test]
    fn sync_clamps_offset_to_last_full_page() {
        // Scrolled deep into tall content, then the content shrinks: the next
        // sync must pull the offset back to the last full page.
        let mut scroll = synced(100, 100, 4);
        assert_eq!(scroll.offset(), 96);
        scroll.sync(10, 4);
        assert_eq!(scroll.offset(), 6);
    }

    #[test]
    fn content_within_view_pins_offset_to_top() {
        let scroll = synced(100, 3, 4);
        assert_eq!(scroll.offset(), 0);
    }

    #[test]
    fn to_bottom_lands_on_last_full_page() {
        let mut scroll = synced(0, 10, 4);
        scroll.scroll_to_bottom();
        assert_eq!(scroll.offset(), 6);
    }

    #[test]
    fn scroll_up_saturates_at_top() {
        let mut scroll = synced(2, 10, 4);
        scroll.scroll_by(VerticalDirection::Up, 9);
        assert_eq!(scroll.offset(), 0);
    }

    #[test]
    fn visible_returns_rows_at_offset() {
        let scroll = synced(2, 10, 3);
        let rows: Vec<usize> = (0..10).collect();
        assert_eq!(scroll.visible(rows), vec![2, 3, 4]);
    }
}
