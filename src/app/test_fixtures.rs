use rusqlite::Connection;
use std::path::Path;

pub(super) fn database(path: &Path) {
    let db = Connection::open(path).unwrap();
    db.execute_batch(include_str!("test_fixtures.sql")).unwrap();
    db.execute(
        "UPDATE media_items SET begins_at=?1",
        [chrono::Utc::now().timestamp() + 1800],
    )
    .unwrap();
}
