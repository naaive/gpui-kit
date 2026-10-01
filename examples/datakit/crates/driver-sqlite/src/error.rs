use datakit_driver::DatabaseError;
use rusqlite::ffi;

/// `error` as a [`DatabaseError`] when SQLite reported it, so the console can
/// show the code and, for a syntax error, point at the position; otherwise
/// as it is.
///
/// The code is SQLite's symbolic name for the extended result code, such as
/// `SQLITE_CONSTRAINT_UNIQUE` or `SQLITE_INTERRUPT`, which is what SQLite's
/// documentation and every forum answer use.
///
/// `sql` is the whole text the driver was given. SQLite reports a syntax
/// error's offset into the statement it was preparing, which is the tail of
/// `sql` when several statements were sent together; the position is moved
/// back into `sql`.
pub(crate) fn convert(error: rusqlite::Error, sql: &str) -> anyhow::Error {
    match error {
        rusqlite::Error::SqliteFailure(failure, message) => {
            let message = message.unwrap_or_else(|| failure.to_string());
            anyhow::Error::new(
                DatabaseError::new(message).with_code(code_name(failure.extended_code)),
            )
        }
        rusqlite::Error::SqlInputError {
            error,
            msg,
            sql: statement,
            offset,
        } => {
            let mut converted = DatabaseError::new(msg).with_code(code_name(error.extended_code));
            if let Ok(offset) = usize::try_from(offset) {
                let start = sql.len().saturating_sub(statement.len());
                let position = (start + offset).min(sql.len());
                if sql.is_char_boundary(position) {
                    converted = converted.with_position(position);
                }
            }
            anyhow::Error::new(converted)
        }
        error => anyhow::Error::new(error),
    }
}

/// SQLite's name for the result code `code`, extended or primary.
pub(crate) fn code_name(code: i32) -> String {
    macro_rules! names {
        ($($name:ident),* $(,)?) => {
            match code {
                $(ffi::$name => return stringify!($name).to_string(),)*
                _ => {}
            }
        };
    }
    names!(
        SQLITE_ERROR_MISSING_COLLSEQ,
        SQLITE_ERROR_RETRY,
        SQLITE_ERROR_SNAPSHOT,
        SQLITE_ABORT_ROLLBACK,
        SQLITE_BUSY_RECOVERY,
        SQLITE_BUSY_SNAPSHOT,
        SQLITE_BUSY_TIMEOUT,
        SQLITE_CANTOPEN_CONVPATH,
        SQLITE_CANTOPEN_DIRTYWAL,
        SQLITE_CANTOPEN_FULLPATH,
        SQLITE_CANTOPEN_ISDIR,
        SQLITE_CANTOPEN_NOTEMPDIR,
        SQLITE_CANTOPEN_SYMLINK,
        SQLITE_CONSTRAINT_CHECK,
        SQLITE_CONSTRAINT_COMMITHOOK,
        SQLITE_CONSTRAINT_FOREIGNKEY,
        SQLITE_CONSTRAINT_FUNCTION,
        SQLITE_CONSTRAINT_NOTNULL,
        SQLITE_CONSTRAINT_PINNED,
        SQLITE_CONSTRAINT_PRIMARYKEY,
        SQLITE_CONSTRAINT_ROWID,
        SQLITE_CONSTRAINT_TRIGGER,
        SQLITE_CONSTRAINT_UNIQUE,
        SQLITE_CONSTRAINT_VTAB,
        SQLITE_CORRUPT_INDEX,
        SQLITE_CORRUPT_SEQUENCE,
        SQLITE_CORRUPT_VTAB,
        SQLITE_IOERR_ACCESS,
        SQLITE_IOERR_CORRUPTFS,
        SQLITE_IOERR_DELETE,
        SQLITE_IOERR_FSYNC,
        SQLITE_IOERR_LOCK,
        SQLITE_IOERR_NOMEM,
        SQLITE_IOERR_READ,
        SQLITE_IOERR_SHORT_READ,
        SQLITE_IOERR_WRITE,
        SQLITE_LOCKED_SHAREDCACHE,
        SQLITE_LOCKED_VTAB,
        SQLITE_READONLY_CANTINIT,
        SQLITE_READONLY_CANTLOCK,
        SQLITE_READONLY_DBMOVED,
        SQLITE_READONLY_DIRECTORY,
        SQLITE_READONLY_RECOVERY,
        SQLITE_READONLY_ROLLBACK,
        SQLITE_ERROR,
        SQLITE_INTERNAL,
        SQLITE_PERM,
        SQLITE_ABORT,
        SQLITE_BUSY,
        SQLITE_LOCKED,
        SQLITE_NOMEM,
        SQLITE_READONLY,
        SQLITE_INTERRUPT,
        SQLITE_IOERR,
        SQLITE_CORRUPT,
        SQLITE_NOTFOUND,
        SQLITE_FULL,
        SQLITE_CANTOPEN,
        SQLITE_PROTOCOL,
        SQLITE_EMPTY,
        SQLITE_SCHEMA,
        SQLITE_TOOBIG,
        SQLITE_CONSTRAINT,
        SQLITE_MISMATCH,
        SQLITE_MISUSE,
        SQLITE_NOLFS,
        SQLITE_AUTH,
        SQLITE_FORMAT,
        SQLITE_RANGE,
        SQLITE_NOTADB,
        SQLITE_NOTICE,
        SQLITE_WARNING,
    );
    // An extended code SQLite added after this list: name its primary code,
    // which is the low byte.
    let primary = code & 0xff;
    if primary != code {
        return format!("{} ({code})", code_name(primary));
    }
    code.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_named_as_sqlite_names_them() {
        assert_eq!(
            code_name(ffi::SQLITE_CONSTRAINT_UNIQUE),
            "SQLITE_CONSTRAINT_UNIQUE"
        );
        assert_eq!(code_name(ffi::SQLITE_INTERRUPT), "SQLITE_INTERRUPT");
        assert_eq!(code_name(19 | (99 << 8)), "SQLITE_CONSTRAINT (25363)");
    }

    #[test]
    fn a_syntax_error_points_into_the_whole_text() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let sql = "SELECT 1; SELECT nope";
        let tail = &sql[10..];
        let error = connection.prepare(tail).unwrap_err();
        let error = convert(error, sql);
        let error = error.downcast_ref::<DatabaseError>().unwrap();
        assert_eq!(error.code(), Some("SQLITE_ERROR"));
        assert_eq!(&sql[error.position().unwrap()..], "nope");
    }
}
