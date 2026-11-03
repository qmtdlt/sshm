//! Offline release metadata signer. Secret seed comes only from the environment.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use sshm::update::{Artifact, Manifest};
use std::{fs, path::Path};

fn main() -> Result<()> {
    let secret = std::env::var("SSHM_UPDATE_SIGNING_KEY")
        .context("SSHM_UPDATE_SIGNING_KEY is required (base64 32-byte Ed25519 seed)")?;
    let seed: [u8; 32] = STANDARD
        .decode(secret)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid signing seed length"))?;
    let key = SigningKey::from_bytes(&seed);
    if std::env::args().nth(1).as_deref() == Some("--public-key") {
        println!("{}", STANDARD.encode(key.verifying_key().as_bytes()));
        return Ok(());
    }
    ensure!(
        std::env::var("SSHM_UPDATE_PUBLIC_KEY")? == STANDARD.encode(key.verifying_key().as_bytes()),
        "signer does not match the embedded public key"
    );
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    let mut artifacts = Vec::new();
    for target in std::env::args().skip(1) {
        let (os, arch, extension) = if target == "windows-x86_64" {
            ("windows", "x86_64".to_owned(), "exe")
        } else {
            ("macos", target, "pkg")
        };
        ensure!(
            matches!(arch.as_str(), "aarch64" | "x86_64"),
            "unsupported architecture"
        );
        let name = format!("sshm-{version}-{os}-{arch}.{extension}");
        let bytes = fs::read(Path::new("dist").join(&name))?;
        artifacts.push(Artifact {
            os: os.into(),
            arch,
            min_macos: if os == "macos" { "11.0" } else { "0" }.into(),
            url: format!("https://github.com/qmtdlt/sshm/releases/download/v{version}/{name}"),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        });
    }
    ensure!(!artifacts.is_empty(), "pass at least one architecture");
    let mut seen = std::collections::HashSet::new();
    for artifact in &artifacts {
        ensure!(
            seen.insert((&artifact.os, &artifact.arch)),
            "duplicate platform"
        );
    }
    // Separate feeds preserve compatibility with macOS 0.4.0, which rejects .exe
    // URLs even for artifacts it would not select. Both use the SAME release key.
    for (os, filename) in [("macos", "update.json"), ("windows", "update-windows.json")] {
        let selected: Vec<_> = artifacts.iter().filter(|a| a.os == os).cloned().collect();
        if selected.is_empty() {
            continue;
        }
        let payload = serde_json::to_vec(&Manifest {
            schema: 1,
            version: version.clone(),
            artifacts: selected,
        })?;
        let envelope = serde_json::json!({"payload": STANDARD.encode(&payload), "signature": STANDARD.encode(key.sign(&payload).to_bytes())});
        fs::write(
            Path::new("dist").join(filename),
            serde_json::to_vec_pretty(&envelope)?,
        )?;
        println!("Wrote dist/{filename}");
    }
    Ok(())
}
