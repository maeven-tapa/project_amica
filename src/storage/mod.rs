use std::{env, fs, path::PathBuf, time::Duration};

use rusqlite::Connection;

pub fn open() -> Result<Connection, Box<dyn std::error::Error>> {
    let path = if let Some(path) = env::var_os("AMICA_DATA_DIR") {
        PathBuf::from(path)
    } else {
        PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unset")?).join("Amica")
    };
    fs::create_dir_all(&path)?;
    let connection = Connection::open(path.join("chat.sqlite3"))?;
    connection.busy_timeout(Duration::from_secs(3))?;
    Ok(connection)
}
