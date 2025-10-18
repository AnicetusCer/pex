use pex::config::local_library_db_path;
use rusqlite::{types::ValueRef, Connection, OptionalExtension, Result};
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() == 1 || args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    let db_path = local_library_db_path();
    if !Path::new(&db_path).exists() {
        eprintln!(
            "Library DB not found at {}. Configure `plex_library_db_source` and launch Pex once to sync it.",
            db_path.display()
        );
        std::process::exit(1);
    }

    let conn = Connection::open(&db_path)?;
    println!("Opening Plex library DB: {}", db_path.display());

    if args.iter().any(|a| a == "--tables") {
        list_tables(&conn)?;
        return Ok(());
    }

    if let Some(idx) = args.iter().position(|a| a == "--schema") {
        let table = args.get(idx + 1).cloned().unwrap_or_else(|| {
            eprintln!("Missing table name after --schema");
            std::process::exit(1);
        });
        show_schema(&conn, &table)?;
        return Ok(());
    }

    // Fall back to table sample behaviour: <table> [limit] [--out file]
    if args.len() < 2 {
        eprintln!("Usage: cargo run --bin library_db_explorer <table> [limit] [--out file]");
        std::process::exit(1);
    }

    let table = &args[1];
    let limit_idx = if args.len() > 2 && !args[2].starts_with("--") {
        Some(2)
    } else {
        None
    };

    let limit: usize = limit_idx
        .and_then(|i| args.get(i))
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let out_file = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1).cloned());

    sample_table(&conn, table, limit, out_file.as_deref())
}

fn print_usage() {
    println!(
        r#"Plex library DB explorer.

Usage:
  cargo run --bin library_db_explorer -- --tables
      List tables in the synced library database.

  cargo run --bin library_db_explorer -- --schema <table>
      Show CREATE TABLE SQL for <table>.

  cargo run --bin library_db_explorer -- <table> [limit] [--out file]
      Dump rows from <table> (default limit = 5).
      Use --out to write the output to a file."#
    );
}

fn list_tables(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], |row| Ok(row.get::<_, String>(0)?))?
        .collect::<Result<Vec<_>, _>>()?;

    println!("Tables ({}):", rows.len());
    for name in rows {
        println!(" - {}", name);
    }
    Ok(())
}

fn show_schema(conn: &Connection, table: &str) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT sql FROM sqlite_master WHERE type='table' AND name = ?1")?;
    let sql: Option<String> = stmt.query_row([table], |row| row.get(0)).optional()?;

    match sql {
        Some(sql) => {
            println!("Schema for `{}`:\n{}\n", table, sql);
        }
        None => {
            println!("No schema found for `{}`.", table);
        }
    }
    Ok(())
}

fn sample_table(
    conn: &Connection,
    table: &str,
    limit: usize,
    out_file: Option<&str>,
) -> Result<()> {
    let sql = format!("SELECT * FROM {} LIMIT {}", table, limit);
    let mut stmt = conn.prepare(&sql)?;

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
                ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).to_string(),
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
        let mut file = File::create(path).expect("Failed to create output file");
        file.write_all(output.as_bytes())
            .expect("Failed to write output file");
        println!("Exported results to {}", path);
    } else {
        print!("{}", output);
    }

    Ok(())
}
