// Release builds: the apk is signed with the key of the script
use std::path::{Path, PathBuf};

use crate::language::runtime::Arguments;

/// The key of release apks, from keystore, key_alias, keystore_password_env and key_password_env
#[derive(Debug, Clone)]
pub struct ReleaseKey {
    pub keystore: PathBuf,
    pub alias: String,
    /// environment variables with the passwords, the script only names them
    pub keystore_password_env: String,
    pub key_password_env: String,
}

impl ReleaseKey {
    /// None without a keystore. The keystore is relative to the script.
    pub fn read(args: &Arguments, project_dir: &Path) -> Result<Option<Self>, String> {
        let Some(keystore) = args.get::<Option<String>>("keystore")? else {
            return Ok(None);
        };

        let alias: Option<String> = args.get("key_alias")?;
        let keystore_password_env: Option<String> = args.get("keystore_password_env")?;

        let (Some(alias), Some(keystore_password_env)) = (alias, keystore_password_env) else {
            return Err("a keystore needs key_alias and keystore_password_env".to_string());
        };

        // the key often has the password of the keystore
        let key_password_env = args
            .get::<Option<String>>("key_password_env")?
            .unwrap_or_else(|| keystore_password_env.clone());

        for name in [&keystore_password_env, &key_password_env] {
            // a password given by mistake must not end up in the error
            let is_name = name.chars().all(|char| char.is_ascii_alphanumeric() || char == '_')
                && !name.starts_with(|char: char| char.is_ascii_digit());

            if !is_name {
                return Err(
                    "keystore_password_env and key_password_env take names of environment variables, not passwords"
                        .to_string(),
                );
            }

            if std::env::var_os(name).is_none() {
                return Err(format!(
                    "the password of the release key comes from {name}, which is not set"
                ));
            }
        }

        let keystore = project_dir.join(keystore);

        if !keystore.is_file() {
            return Err(format!("the keystore {} does not exist", keystore.display()));
        }

        return Ok(Some(ReleaseKey {
            keystore,
            alias,
            keystore_password_env,
            key_password_env,
        }));
    }
}
