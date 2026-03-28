use std::sync::Arc;

#[derive(Debug)]
pub enum GameError {
    FilesystemError(String),
    ResourceLoadError(String),
    ResourceNotFound(String, Vec<(std::path::PathBuf, GameError)>),
    #[allow(clippy::upper_case_acronyms)]
    IOError(Arc<std::io::Error>),
    CustomError(String),
}

impl From<std::io::Error> for GameError {
    fn from(e: std::io::Error) -> GameError {
        GameError::IOError(Arc::new(e))
    }
}

impl From<zip::result::ZipError> for GameError {
    fn from(e: zip::result::ZipError) -> GameError {
        let errstr = format!("Zip error: {e}");
        GameError::ResourceLoadError(errstr)
    }
}
