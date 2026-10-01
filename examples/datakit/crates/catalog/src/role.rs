use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A user or role of the database server: who can connect, and whose
/// privileges it inherits.
///
/// Servers draw the line between users and roles differently — PostgreSQL
/// has only roles, some of which can log in — so a role here is anything
/// privileges are granted to, and [`Role::can_login`] tells users apart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    name: Arc<str>,
    can_login: bool,
    /// What the server says it may do, in its own words: `SUPERUSER`,
    /// `CREATEDB`, `host %`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attributes: Vec<Arc<str>>,
    /// The roles it is a member of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    member_of: Vec<Arc<str>>,
    /// The statements that create it as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    definition: Option<Arc<str>>,
}

impl Role {
    pub fn new(name: impl Into<Arc<str>>, can_login: bool) -> Self {
        Self {
            name: name.into(),
            can_login,
            attributes: Vec::new(),
            member_of: Vec::new(),
            definition: None,
        }
    }

    pub fn with_attributes(mut self, attributes: impl IntoIterator<Item = Arc<str>>) -> Self {
        self.attributes = attributes.into_iter().collect();
        self
    }

    pub fn with_member_of(mut self, roles: impl IntoIterator<Item = Arc<str>>) -> Self {
        self.member_of = roles.into_iter().collect();
        self
    }

    pub fn with_definition(mut self, definition: impl Into<Arc<str>>) -> Self {
        self.definition = Some(definition.into());
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn can_login(&self) -> bool {
        self.can_login
    }

    pub fn attributes(&self) -> &[Arc<str>] {
        &self.attributes
    }

    pub fn member_of(&self) -> &[Arc<str>] {
        &self.member_of
    }

    pub fn definition(&self) -> Option<&str> {
        self.definition.as_deref()
    }
}
