use gpui_kit::SharedString;

use super::{ActionPanel, Tone};

/// One object in full: Markdown body, labelled metadata, and actions.
///
/// Used as a page of its own and as the side panel of a list item.
#[derive(Clone, Debug, Default)]
pub struct DetailModel {
    markdown: SharedString,
    metadata: Vec<Metadata>,
    actions: ActionPanel,
    loading: bool,
}

impl DetailModel {
    pub fn new(markdown: impl Into<SharedString>) -> Self {
        Self {
            markdown: markdown.into(),
            ..Self::default()
        }
    }

    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata.push(metadata);
        self
    }

    pub fn with_actions(mut self, actions: ActionPanel) -> Self {
        self.actions = actions;
        self
    }

    pub fn with_loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn markdown(&self) -> &SharedString {
        &self.markdown
    }

    pub fn metadata(&self) -> &[Metadata] {
        &self.metadata
    }

    pub fn actions(&self) -> &ActionPanel {
        &self.actions
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }
}

/// A labelled value in a detail's metadata column. A `None` value draws a
/// separator line.
#[derive(Clone, Debug)]
pub struct Metadata {
    label: SharedString,
    value: MetadataValue,
}

impl Metadata {
    pub fn new(label: impl Into<SharedString>, value: MetadataValue) -> Self {
        Self {
            label: label.into(),
            value,
        }
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn value(&self) -> &MetadataValue {
        &self.value
    }
}

#[derive(Clone, Debug)]
pub enum MetadataValue {
    Text(SharedString),
    Link {
        text: SharedString,
        url: SharedString,
    },
    Tags(Vec<Tag>),
    Separator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    text: SharedString,
    tone: Tone,
}

impl Tag {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Neutral,
        }
    }

    pub fn with_tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn text(&self) -> &SharedString {
        &self.text
    }

    pub fn tone(&self) -> Tone {
        self.tone
    }
}
