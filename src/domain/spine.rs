/// A single item in the reading order, referencing a manifest entry.
#[derive(Debug, Clone)]
pub struct SpineItem {
    /// ID of the manifest item this spine entry refers to.
    pub manifest_id: String,
    /// Whether this item is part of the primary linear reading order.
    pub linear: bool,
}

impl SpineItem {
    pub fn new(manifest_id: impl Into<String>) -> Self {
        Self {
            manifest_id: manifest_id.into(),
            linear: true,
        }
    }

    pub fn non_linear(manifest_id: impl Into<String>) -> Self {
        Self {
            manifest_id: manifest_id.into(),
            linear: false,
        }
    }
}

/// Page progression direction for the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageProgression {
    Ltr,
    Rtl,
}

/// The spine defines the default reading order of the book.
#[derive(Debug, Clone, Default)]
pub struct Spine {
    pub items: Vec<SpineItem>,
    pub page_progression_direction: Option<PageProgression>,
}

impl Spine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, item: SpineItem) {
        self.items.push(item);
    }

    /// Adds a linear spine item referencing the given manifest ID.
    pub fn add(&mut self, manifest_id: impl Into<String>) {
        self.items.push(SpineItem::new(manifest_id));
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SpineItem> {
        self.items.iter()
    }

    /// Returns only the linear items in reading order.
    pub fn linear_items(&self) -> impl Iterator<Item = &SpineItem> {
        self.items.iter().filter(|item| item.linear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spine_item_defaults_to_linear() {
        let item = SpineItem::new("ch1");
        assert!(item.linear);
        assert_eq!(item.manifest_id, "ch1");
    }

    #[test]
    fn non_linear_spine_item() {
        let item = SpineItem::non_linear("appendix");
        assert!(!item.linear);
    }

    #[test]
    fn linear_items_filters_correctly() {
        let mut spine = Spine::new();
        spine.push(SpineItem::new("ch1"));
        spine.push(SpineItem::non_linear("notes"));
        spine.push(SpineItem::new("ch2"));

        let linear: Vec<_> = spine.linear_items().collect();
        assert_eq!(linear.len(), 2);
        assert_eq!(linear[0].manifest_id, "ch1");
        assert_eq!(linear[1].manifest_id, "ch2");
    }
}
