#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("token format is invalid")]
	InvalidToken,
	#[error("no session was found that matches the requested token")]
	NotFound,
}
