//! Database file and connection setup (docs/23 T2.5).

use super::*;


pub(super) fn open_writer(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(&connection)?;
    Ok(connection)
}
