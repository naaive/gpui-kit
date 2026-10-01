use std::{fmt, sync::Arc};

/// An error the database reported about a statement, with the parts a person
/// needs to fix it.
///
/// Drivers return it inside `anyhow::Error`; the console downcasts to place
/// [`position`](Self::position) in the editor and to show the detail and hint
/// under the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseError {
    message: Arc<str>,
    code: Option<Arc<str>>,
    detail: Option<Arc<str>>,
    hint: Option<Arc<str>>,
    position: Option<usize>,
}

impl DatabaseError {
    pub fn new(message: impl Into<Arc<str>>) -> Self {
        Self {
            message: message.into(),
            code: None,
            detail: None,
            hint: None,
            position: None,
        }
    }

    /// The database's error code, such as a SQLSTATE.
    pub fn with_code(mut self, code: impl Into<Arc<str>>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<Arc<str>>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<Arc<str>>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Where in the statement the error is, as a byte offset into the text
    /// the driver was given.
    pub fn with_position(mut self, position: usize) -> Self {
        self.position = Some(position);
        self
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    pub fn position(&self) -> Option<usize> {
        self.position
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DatabaseError {}
