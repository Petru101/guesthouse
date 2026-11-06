//! The one error type: something the user can be told about in plain words.

use std::fmt;
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub String);

pub type Result<T> = std::result::Result<T, Failure>;

impl Failure {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}
