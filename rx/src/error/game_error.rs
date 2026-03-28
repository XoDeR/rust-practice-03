#[derive(Debug)]
pub enum GameError {
    FilesystemError(String),
    CustomError(String),
}
