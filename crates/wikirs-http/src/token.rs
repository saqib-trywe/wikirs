//! The `serve` bearer token (http-security.md#authentication): one per Wiki,
//! in its state dir outside the Wiki, so no sync tool carries it.

use std::{io::Write, path::PathBuf};

use wikirs_core::Wiki;

fn path(wiki: &Wiki) -> PathBuf {
    wiki.state_dir().join("serve-token")
}

/// The Wiki's token, if one has been generated.
#[must_use]
pub fn read(wiki: &Wiki) -> Option<String> {
    std::fs::read_to_string(path(wiki))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// The token, generating one if there's none. `true` when it's new.
pub fn get_or_create(wiki: &Wiki) -> anyhow::Result<(String, bool)> {
    match read(wiki) {
        Some(token) => Ok((token, false)),
        None => Ok((rotate(wiki)?, true)),
    }
}

/// Replaces the token with a fresh random one (256 bits, hex), readable
/// only by this user.
pub fn rotate(wiki: &Wiki) -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness for a token: {e}"))?;
    let token: String = bytes.iter().fold(String::with_capacity(64), |mut hex, b| {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
        hex
    });
    let target = path(wiki);
    std::fs::create_dir_all(wiki.state_dir())?;
    let temp = target.with_extension("tmp");
    let mut options = std::fs::File::options();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temp)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, &target)?;
    Ok(token)
}

/// Compares in time independent of where the inputs differ.
#[must_use]
pub fn matches(given: &str, token: &str) -> bool {
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_created_once_rotated_on_demand_and_private() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wiki")).unwrap();
        let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("base")).unwrap();
        assert_eq!(read(&wiki), None);
        let (first, new) = get_or_create(&wiki).unwrap();
        assert!(new);
        assert_eq!(first.len(), 64);
        assert_eq!(get_or_create(&wiki).unwrap(), (first.clone(), false));
        assert!(
            !path(&wiki).starts_with(wiki.root()),
            "never inside the Wiki"
        );
        let second = rotate(&wiki).unwrap();
        assert_ne!(first, second);
        assert_eq!(read(&wiki), Some(second.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(&wiki)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(matches(&second, &second));
        assert!(!matches(&first, &second));
        assert!(!matches("", &second));
    }
}
