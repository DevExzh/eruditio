/// An item in the table of contents. Supports hierarchical nesting.
#[derive(Debug, Clone)]
pub struct TocItem {
    pub title: String,
    /// Relative href within the book (e.g. "chapter1.xhtml#section2").
    pub href: String,
    /// Nested sub-items for sub-sections.
    pub children: Vec<TocItem>,
    /// Optional unique identifier.
    pub id: Option<String>,
    /// Optional play order (sequential index for NCX compatibility).
    pub play_order: Option<u32>,
}

impl TocItem {
    pub fn new(title: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            href: href.into(),
            children: Vec::new(),
            id: None,
            play_order: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_play_order(mut self, order: u32) -> Self {
        self.play_order = Some(order);
        self
    }

    pub fn with_children(mut self, children: Vec<TocItem>) -> Self {
        self.children = children;
        self
    }

    /// Returns the total number of items in this subtree (including self).
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(TocItem::count).sum::<usize>()
    }

    /// Flattens the TOC tree into a depth-first iterator of (depth, &TocItem).
    pub fn flatten(&self) -> Vec<(usize, &TocItem)> {
        let mut result = vec![(0, self)];
        for child in &self.children {
            for (depth, item) in child.flatten() {
                result.push((depth + 1, item));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toc_item_builder() {
        let item = TocItem::new("Chapter 1", "ch1.xhtml")
            .with_id("navpoint-1")
            .with_play_order(1);

        assert_eq!(item.title, "Chapter 1");
        assert_eq!(item.href, "ch1.xhtml");
        assert_eq!(item.id.as_deref(), Some("navpoint-1"));
        assert_eq!(item.play_order, Some(1));
    }

    #[test]
    fn count_includes_children() {
        let item = TocItem::new("Part 1", "part1.xhtml").with_children(vec![
            TocItem::new("Chapter 1", "ch1.xhtml"),
            TocItem::new("Chapter 2", "ch2.xhtml")
                .with_children(vec![TocItem::new("Section 2.1", "ch2.xhtml#s1")]),
        ]);
        assert_eq!(item.count(), 4);
    }

    #[test]
    fn flatten_produces_depth_first_order() {
        let root = TocItem::new("Root", "root.xhtml").with_children(vec![
            TocItem::new("A", "a.xhtml"),
            TocItem::new("B", "b.xhtml").with_children(vec![TocItem::new("B1", "b1.xhtml")]),
        ]);
        let flat = root.flatten();
        assert_eq!(flat.len(), 4);
        assert_eq!(flat[0].0, 0); // Root at depth 0
        assert_eq!(flat[1].0, 1); // A at depth 1
        assert_eq!(flat[2].0, 1); // B at depth 1
        assert_eq!(flat[3].0, 2); // B1 at depth 2
    }
}
