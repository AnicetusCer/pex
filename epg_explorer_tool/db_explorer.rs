use pex::config::load_config;
use rusqlite::{Connection, Result, types::ValueRef};
use std::env;
use std::fs::File;
use std::io::Write;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: cargo run --bin db_explorer <table> [limit] [--out file]");
        std::process::exit(1);
    }

    let table = &args[1];
    let limit: usize = if args.len() > 2 && !args[2].starts_with("--") {
        args[2].parse().unwrap_or(5)
    } else {
        5
    };

    let out_file: Option<String> = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1).cloned());

    println!("Opening Plex DB: plex_epg.db");

    let config = load_config();
    let db_path = if config.plex_db_local.is_empty() {
        "plex_epg.db".to_string()
    } else {
        config.plex_db_local
    };
    let conn = Connection::open(&db_path)?;
    let mut stmt = conn.prepare(&format!("SELECT * FROM {} LIMIT {}", table, limit))?;

    // Grab column names now so stmt borrow is released
    let column_names: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    let rows = stmt.query_map([], |row| {
        let mut values: Vec<String> = Vec::with_capacity(column_names.len());

        for (i, _) in column_names.iter().enumerate() {
            let value = match row.get_ref(i).unwrap() {
                ValueRef::Null => "NULL".to_string(),
                ValueRef::Integer(i) => i.to_string(),
                ValueRef::Real(f) => f.to_string(),
                ValueRef::Text(bytes) => {
                    let raw = String::from_utf8_lossy(bytes).to_string();
                    raw
                }
                ValueRef::Blob(_) => "<BLOB>".to_string(),
            };
            values.push(value);
        }

        Ok(values)
    })?;

    let mut output = String::new();
    output.push_str(&format!("--- Table: {} ---\n", table));
    output.push_str(&format!("Columns: {:?}\n", column_names));

    for row in rows {
        output.push_str(&format!("{:?}\n", row?));
    }

    if let Some(path) = out_file {
        let mut file = File::create(&path).expect("Failed to create output file");
        file.write_all(output.as_bytes())
            .expect("Failed to write output file");
        println!("Exported results to {}", path);
    } else {
        print!("{}", output);
    }

    Ok(())
}
