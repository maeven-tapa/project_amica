use std::env;

#[cfg(windows)]
use slint::ComponentHandle;

#[cfg(windows)]
fn entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new("Project Amica", "Gemini API key")
}

fn environment_key() -> Option<String> {
    env::var("GEMINI_API_KEY")
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

pub fn load() -> Result<String, &'static str> {
    if let Some(key) = environment_key() {
        return Ok(key);
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

#[cfg(windows)]
fn store(key: &str) -> Result<(), &'static str> {
    let key = key.trim();
    if key.is_empty() || key.bytes().any(|byte| !(33..127).contains(&byte)) {
        return Err("Enter a Gemini API key without spaces.");
    }
    entry()
        .and_then(|credential| credential.set_password(key))
        .map_err(|_| "Amica could not save the Gemini key in Windows Credential Manager.")
}

pub(super) fn connect_setup(window: &crate::AmicaWindow) {
    let from_environment = environment_key().is_some();
    let configured = from_environment || load().is_ok();
    window.set_gemini_setup_available(true);
    window.set_gemini_key_can_save(cfg!(windows));
    window.set_gemini_key_configured(configured);
    window.set_gemini_key_from_environment(from_environment);

    #[cfg(windows)]
    {
        let weak = window.as_weak();
        window.on_save_gemini_key(move |key| {
            let Some(window) = weak.upgrade() else {
                return "".into();
            };
            match store(&key) {
                Ok(()) => {
                    window.set_gemini_key_configured(true);
                    "".into()
                }
                Err(error) => error.into(),
            }
        });
    }

    if !configured {
        window.invoke_show_panel(crate::Panel::Settings);
    }
}

pub fn store_from_stdin() -> Result<(), &'static str> {
    #[cfg(windows)]
    {
        let mut key = String::new();
        std::io::stdin()
            .read_line(&mut key)
            .map_err(|_| "Could not read the Gemini API key.")?;
        store(&key)
    }
    #[cfg(not(windows))]
    Err("Set the GEMINI_API_KEY environment variable on this platform.")
}
