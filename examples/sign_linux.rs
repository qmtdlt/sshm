//! Sign Linux checksums with the existing release key; never print the seed.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::fs;

fn main() -> Result<()> {
    let seed: [u8; 32] = STANDARD
        .decode(
            std::env::var("SSHM_UPDATE_SIGNING_KEY")
                .context("SSHM_UPDATE_SIGNING_KEY is required")?
                .trim(),
        )?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid signing seed length"))?;
    let key = SigningKey::from_bytes(&seed);
    let installer = fs::read_to_string("install.sh")?;
    let encoded: String = installer
        .split("-----BEGIN PUBLIC KEY-----")
        .nth(1)
        .context("installer public key is missing")?
        .split("-----END PUBLIC KEY-----")
        .next()
        .unwrap()
        .split_whitespace()
        .collect();
    let der = STANDARD.decode(encoded)?;
    let mut expected = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    expected.extend_from_slice(key.verifying_key().as_bytes());
    ensure!(
        der == expected,
        "release key does not match the installer's pinned public key"
    );
    let payload = fs::read("dist/linux/linux-SHA256SUMS")?;
    let archive = fs::read("dist/linux/sshm-linux-x86_64.tar.gz")?;
    let expected_payload = format!(
        "version={}\n{:x}  sshm-linux-x86_64.tar.gz\n",
        env!("CARGO_PKG_VERSION"),
        Sha256::digest(&archive)
    );
    ensure!(
        payload == expected_payload.as_bytes(),
        "checksum manifest differs from Cargo version or archive"
    );
    fs::write(
        "dist/linux/linux-SHA256SUMS.sig",
        key.sign(&payload).to_bytes(),
    )?;
    println!("Signed dist/linux/linux-SHA256SUMS");
    Ok(())
}
