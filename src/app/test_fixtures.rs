use rusqlite::Connection;
use std::path::Path;

pub(super) fn database(path: &Path) {
    let db = Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE metadata_items(
        id INTEGER PRIMARY KEY, title TEXT, original_title TEXT, year INTEGER,
        metadata_type INTEGER, guid TEXT, updated_at INTEGER, added_at INTEGER,
        user_thumb_url TEXT, thumb_url TEXT, tags_genre TEXT, summary TEXT,
        audience_rating REAL, rating REAL);
        CREATE TABLE media_items(id INTEGER PRIMARY KEY, metadata_item_id INTEGER,
        width INTEGER, height INTEGER, updated_at INTEGER, begins_at INTEGER, extra_data TEXT);
        CREATE TABLE media_parts(id INTEGER PRIMARY KEY, media_item_id INTEGER,
        file TEXT, size INTEGER, updated_at INTEGER);
        INSERT INTO metadata_items VALUES(1,'Fixture Movie','Fixture Movie',2020,1,'imdb://tt1234567',100,100,
        'https://fixture.invalid/poster.jpg',NULL,'Drama','Fixture synopsis',7,7);
        INSERT INTO media_items VALUES(1,1,1920,1080,100,0,NULL);
        INSERT INTO media_parts VALUES(1,1,'/fixture/Fixture Movie (2020).mkv',1000,100);")
        .unwrap();
    db.execute(
        "UPDATE media_items SET begins_at=?1",
        [chrono::Utc::now().timestamp() + 1800],
    )
    .unwrap();
}
