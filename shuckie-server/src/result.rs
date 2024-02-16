use std::fmt::{Display, Formatter};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    ThreadAlreadyExists,
    NotStarted,
    IO(std::io::Error)
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ThreadAlreadyExists => f.write_str("thread already exists"),
            Self::NotStarted => f.write_str("server not started"),
            Self::IO(e) => f.write_fmt(format_args!("I/O error: {e:?}"))
        }
    }
}

pub(crate) fn io<T>(what: std::io::Result<T>) -> Result<T> {
    what.map_err(|e| Error::IO(e))
}
