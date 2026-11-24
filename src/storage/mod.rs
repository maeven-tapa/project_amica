use std::{env, fs, path::PathBuf, time::Duration};

use rusqlite::Connection;

pub fn open() -> Result<Connection, Box<dyn std::error::Error>> {
    let path = env::var_os("AMICA_DATA_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .map_or_else(default_data_dir, Ok)?;
    fs::create_dir_all(&path)?;
    let connection = Connection::open(path.join("chat.sqlite3"))?;
    connection.busy_timeout(Duration::from_secs(3))?;
    Ok(connection)
}

#[cfg(windows)]
fn default_data_dir() -> Result<PathBuf, &'static str> {
    Ok(PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unset")?).join("Amica"))
}

#[cfg(not(windows))]
fn default_data_dir() -> Result<PathBuf, &'static str> {
    if let Some(path) = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(path.join("amica"));
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("HOME is unavailable. Set AMICA_DATA_DIR to a writable directory.")?;
    Ok(home.join(".local/share/amica"))
}
