use std::env;

#[cfg(windows)]
fn entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new("Project Amica", "Gemini API key")
}

pub fn load() -> Result<String, &'static str> {
    if let Ok(key) = env::var("GEMINI_API_KEY")
        && !key.trim().is_empty()
    {
        return Ok(key.trim().to_owned());
    }

    #[cfg(windows)]
    match entry().and_then(|credential| credential.get_password()) {
        Ok(key) => return Ok(key),
        Err(keyring::Error::NoEntry) => (),
        Err(_) => {
            return Err("Amica could not read the Gemini key from Windows Credential Manager.");
        }
    }

    Err("Set a Gemini API key to start chatting with Amica.")
}

pub fn store_from_stdin() -> Result<(), &'static str> {
    #[cfg(windows)]
    {
        let mut key = String::new();
        std::io::stdin()
            .read_line(&mut key)
            .map_err(|_| "Could not read the Gemini API key.")?;
        let key = key.trim();
        if key.is_empty() || key.bytes().any(|byte| !(33..127).contains(&byte)) {
            return Err("Enter a Gemini API key without spaces.");
        }
        entry()
            .and_then(|credential| credential.set_password(key))
            .map_err(|_| "Amica could not save the Gemini key in Windows Credential Manager.")
    }
    #[cfg(not(windows))]
    Err("Set the GEMINI_API_KEY environment variable on this platform.")
}
