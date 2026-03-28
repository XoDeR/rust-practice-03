use crate::error::GameError;

pub type GameResult<T = ()> = Result<T, GameError>;
