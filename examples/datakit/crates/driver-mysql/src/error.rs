use datakit_driver::DatabaseError;

/// The server's error for an empty statement, such as one that is only a
/// comment.
const EMPTY_QUERY: u16 = 1065;

/// `error` as a [`DatabaseError`] when the server reported it; otherwise as
/// it is.
///
/// The code is the SQLSTATE, as for every other database; MySQL's own error
/// number, which its manual and forums are indexed by, is the detail
/// (`Error 1146`). MySQL does not say where in the statement an error is, so
/// there is no position.
pub(crate) fn convert(error: mysql_async::Error) -> anyhow::Error {
    let mysql_async::Error::Server(server) = error else {
        return anyhow::Error::new(error);
    };
    anyhow::Error::new(
        DatabaseError::new(server.message)
            .with_code(server.state)
            .with_detail(format!("Error {}", server.code)),
    )
}

/// Whether the server refused `error`'s statement for being empty.
pub(crate) fn is_empty_query(error: &mysql_async::Error) -> bool {
    matches!(error, mysql_async::Error::Server(server) if server.code == EMPTY_QUERY)
}
