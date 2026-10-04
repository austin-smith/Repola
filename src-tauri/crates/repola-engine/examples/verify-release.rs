//! Verify final release bytes against the same Minisign format used by bootstrap.
use std::{env, fs, path::PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use minisign_verify::{PublicKey, Signature};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let public_key = env::var("REPOLA_SIGNING_PUBLIC_KEY")?;
    let document = String::from_utf8(BASE64.decode(public_key.trim())?)?;
    let public_key = PublicKey::decode(&document)?;
    let mut arguments = env::args_os().skip(1);
    let version = arguments
        .next()
        .ok_or("missing expected release version")?
        .into_string()
        .map_err(|_| "invalid release version")?;
    let paths: Vec<PathBuf> = arguments.map(PathBuf::from).collect();
    if paths.is_empty() {
        return Err("usage: verify-release <version> <artifact>...".into());
    }
    for path in paths {
        let mut signature_path = path.as_os_str().to_os_string();
        signature_path.push(".sig");
        let envelope = fs::read_to_string(signature_path)?;
        let document = String::from_utf8(BASE64.decode(envelope.trim())?)?;
        let signature = Signature::decode(&document)?;
        public_key.verify(&fs::read(&path)?, &signature, false)?;
        let signed_version = signature
            .trusted_comment()
            .split('\t')
            .find_map(|field| field.strip_prefix("version:"));
        if signed_version != Some(version.as_str()) {
            return Err("artifact signature does not bind the expected release version".into());
        }
        println!("verified {}", path.display());
    }
    Ok(())
}
