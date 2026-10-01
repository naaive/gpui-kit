use datakit_driver::DatabaseError;
use tokio_postgres::error::ErrorPosition;

/// `error` as a [`DatabaseError`] when the server reported it, so the console
/// can point at the position; otherwise as it is.
pub(crate) fn convert(error: tokio_postgres::Error, sql: &str) -> anyhow::Error {
    let Some(db) = error.as_db_error() else {
        return anyhow::Error::new(error);
    };
    let mut converted = DatabaseError::new(db.message()).with_code(db.code().code());
    if let Some(detail) = db.detail() {
        converted = converted.with_detail(detail);
    }
    if let Some(hint) = db.hint() {
        converted = converted.with_hint(hint);
    }
    if let Some(ErrorPosition::Original(position)) = db.position() {
        // PostgreSQL counts characters from 1.
        let characters = (*position as usize).saturating_sub(1);
        let byte = sql
            .char_indices()
            .nth(characters)
            .map_or(sql.len(), |(byte, _)| byte);
        converted = converted.with_position(byte);
    }
    anyhow::Error::new(converted)
}
