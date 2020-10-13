use core::fmt::{Debug, Display};
#[cfg(feature = "std")]
use std::error::Error;

#[derive(Debug, Copy, Clone)]
pub enum ParseError {
    MalformedInput,
    UnexpectedEof,
}

#[cfg(feature = "std")]
impl Error for ParseError {}

impl Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::MalformedInput => write!(f, "Malformed input file"),
            ParseError::UnexpectedEof => write!(f, "Unexpected end of file"),
        }
    }
}

impl From<core::num::TryFromIntError> for ParseError {
    fn from(_: core::num::TryFromIntError) -> Self {
        ParseError::MalformedInput
    }
}

impl From<crate::parser::UnexpectedEof> for ParseError {
    fn from(_: crate::parser::UnexpectedEof) -> Self {
        ParseError::UnexpectedEof
    }
}