/// Standard guide reference types (EPUB2 / OPF 2.0 spec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuideType {
    Cover,
    TitlePage,
    Toc,
    Index,
    Glossary,
    Acknowledgements,
    Bibliography,
    Colophon,
    CopyrightPage,
    Dedication,
    Epigraph,
    Foreword,
    Loi,
    Lot,
    Notes,
    Preface,
    Text,
    Other(String),
}

impl std::str::FromStr for GuideType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "cover" => Self::Cover,
            "title-page" => Self::TitlePage,
            "toc" => Self::Toc,
            "index" => Self::Index,
            "glossary" => Self::Glossary,
            "acknowledgements" => Self::Acknowledgements,
            "bibliography" => Self::Bibliography,
            "colophon" => Self::Colophon,
            "copyright-page" => Self::CopyrightPage,
            "dedication" => Self::Dedication,
            "epigraph" => Self::Epigraph,
            "foreword" => Self::Foreword,
            "loi" => Self::Loi,
            "lot" => Self::Lot,
            "notes" => Self::Notes,
            "preface" => Self::Preface,
            "text" => Self::Text,
            other => Self::Other(other.to_string()),
        })
    }
}

impl GuideType {
    /// Returns the canonical string representation.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Cover => "cover",
            Self::TitlePage => "title-page",
            Self::Toc => "toc",
            Self::Index => "index",
            Self::Glossary => "glossary",
            Self::Acknowledgements => "acknowledgements",
            Self::Bibliography => "bibliography",
            Self::Colophon => "colophon",
            Self::CopyrightPage => "copyright-page",
            Self::Dedication => "dedication",
            Self::Epigraph => "epigraph",
            Self::Foreword => "foreword",
            Self::Loi => "loi",
            Self::Lot => "lot",
            Self::Notes => "notes",
            Self::Preface => "preface",
            Self::Text => "text",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// A single guide reference pointing to a semantic section of the book.
#[derive(Debug, Clone)]
pub struct GuideReference {
    pub ref_type: GuideType,
    pub title: String,
    pub href: String,
}

/// The guide: a set of references to standard structural sections.
#[derive(Debug, Clone, Default)]
pub struct Guide {
    pub references: Vec<GuideReference>,
}

impl Guide {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, reference: GuideReference) {
        self.references.push(reference);
    }

    /// Finds the first reference matching the given type.
    pub fn find(&self, ref_type: &GuideType) -> Option<&GuideReference> {
        self.references.iter().find(|r| &r.ref_type == ref_type)
    }

    pub fn is_empty(&self) -> bool {
        self.references.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_type_round_trip() {
        let types = ["cover", "title-page", "toc", "index", "glossary", "text"];
        for t in types {
            let parsed: GuideType = t.parse().unwrap();
            assert_eq!(parsed.as_str(), t);
        }
    }

    #[test]
    fn guide_type_other_preserves_value() {
        let t: GuideType = "custom-type".parse().unwrap();
        assert_eq!(t, GuideType::Other("custom-type".into()));
        assert_eq!(t.as_str(), "custom-type");
    }

    #[test]
    fn guide_find_by_type() {
        let mut guide = Guide::new();
        guide.push(GuideReference {
            ref_type: GuideType::Cover,
            title: "Cover".into(),
            href: "cover.xhtml".into(),
        });
        guide.push(GuideReference {
            ref_type: GuideType::Toc,
            title: "Table of Contents".into(),
            href: "toc.xhtml".into(),
        });

        assert_eq!(guide.find(&GuideType::Cover).unwrap().href, "cover.xhtml");
        assert!(guide.find(&GuideType::Index).is_none());
    }
}
