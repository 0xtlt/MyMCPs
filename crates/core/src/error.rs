use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] sqlx::Error),

    /// The environment or a file it points to cannot be used as it is. The
    /// message is written for whoever starts the server.
    #[error("{0}")]
    Config(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A migration recorded in the database is not one this version knows,
    /// or one of its statements failed.
    #[error("migration {name} failed: {source}")]
    Migration {
        name: String,
        #[source]
        source: sqlx::Error,
    },
}
