use age::secrecy::ExposeSecret;
use age::x25519::{Identity, Recipient};
use anyhow::{Context, Result, anyhow};
use std::io::{Read, Write};
use std::path::PathBuf;

pub struct Keys {
    identity: Identity,
    recipient: Recipient,
}

fn identity_path() -> PathBuf {
    crate::config::home().join("identity.txt")
}

impl Keys {
    /// Load the shared age identity, generating one on first use
    pub fn load_or_create() -> Result<Keys> {
        let p = identity_path();
        let identity = if p.exists() {
            let s = std::fs::read_to_string(&p)?;
            let line = s
                .lines()
                .find(|l| l.starts_with("AGE-SECRET-KEY-"))
                .ok_or_else(|| anyhow!("no AGE-SECRET-KEY in {}", p.display()))?;
            line.parse::<Identity>()
                .map_err(|e| anyhow!("parse identity: {e}"))?
        } else {
            let id = Identity::generate();
            let body = format!(
                "# engram shared identity, copy to every device. public: {}\n{}\n",
                id.to_public(),
                id.to_string().expose_secret()
            );
            crate::config::write_private(&p, body.as_bytes())?;
            id
        };
        let recipient = identity.to_public();
        Ok(Keys {
            identity,
            recipient,
        })
    }

    pub fn public(&self) -> String {
        self.recipient.to_string()
    }

    pub fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let enc = age::Encryptor::with_recipients(std::iter::once(
            &self.recipient as &dyn age::Recipient,
        ))
        .context("encryptor")?;
        let mut out = Vec::with_capacity(plain.len() + 256);
        let mut w = enc.wrap_output(&mut out)?;
        w.write_all(plain)?;
        w.finish()?;
        Ok(out)
    }

    pub fn decrypt(&self, cipher: &[u8]) -> Result<Vec<u8>> {
        let dec = age::Decryptor::new(cipher).context("not an age file")?;
        let mut r = dec
            .decrypt(std::iter::once(&self.identity as &dyn age::Identity))
            .context("decrypt (wrong identity?)")?;
        let mut out = Vec::new();
        r.read_to_end(&mut out)?;
        Ok(out)
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(data))
}
