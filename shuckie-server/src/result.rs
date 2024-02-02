pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    ThreadAlreadyExists,
    NotStarted,
    IO(std::io::Error)
}

pub(crate) fn io<T>(what: std::io::Result<T>) -> Result<T> {
    what.map_err(|e| Error::IO(e))
}
