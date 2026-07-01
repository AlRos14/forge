use std::{fs, io::Write, path::Path};

use crate::{error::ConfigError, ForgeConfig};

const MIN_JWT_SECRET_BYTES: usize = 32;

impl ForgeConfig {
    /// Resolve the JWT signing secret used for auth tokens.
    ///
    /// Precedence:
    /// 1. `server.jwt_secret` from config (file, env, or CLI override)
    /// 2. Existing file at [`ForgeConfig::jwt_secret_path`]
    /// 3. Generate a cryptographically random secret, persist it, and use it
    pub fn resolve_jwt_secret(&self) -> Result<Vec<u8>, ConfigError> {
        if let Some(secret) = &self.server.jwt_secret {
            if secret.is_empty() {
                return Err(ConfigError::InvalidConfig {
                    message: "server.jwt_secret cannot be empty".to_owned(),
                });
            }
            return Ok(secret.as_bytes().to_vec());
        }

        let path = self.jwt_secret_path();
        if path.is_file() {
            let bytes = fs::read(&path).map_err(|source| ConfigError::Read {
                path: path.clone(),
                source,
            })?;
            validate_jwt_secret_bytes(&bytes, &path)?;
            return Ok(bytes);
        }

        let secret = generate_jwt_secret();
        persist_jwt_secret(&path, &secret)?;
        Ok(secret)
    }
}

fn validate_jwt_secret_bytes(bytes: &[u8], path: &Path) -> Result<(), ConfigError> {
    if bytes.len() < MIN_JWT_SECRET_BYTES {
        return Err(ConfigError::InvalidConfig {
            message: format!(
                "JWT secret file {} must be at least {MIN_JWT_SECRET_BYTES} bytes",
                path.display()
            ),
        });
    }
    Ok(())
}

fn generate_jwt_secret() -> Vec<u8> {
    let mut secret = vec![0_u8; MIN_JWT_SECRET_BYTES];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut secret);
    secret
}

fn persist_jwt_secret(path: &Path, secret: &[u8]) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(secret)
            .map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, secret).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }

    Ok(())
}
