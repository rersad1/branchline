//! Verify a public release artifact without running its installer.
use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: verify-update INSTALLER SIGNATURE VERSION".into());
    }
    let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))?;
    let public = String::from_utf8(
        STANDARD.decode(
            config["plugins"]["updater"]["pubkey"]
                .as_str()
                .ok_or("missing public key")?,
        )?,
    )?;
    let signature = String::from_utf8(STANDARD.decode(std::fs::read_to_string(&args[1])?.trim())?)?;
    let key = PublicKey::decode(&public)?;
    let signature = Signature::decode(&signature)?;
    let mut artifact = std::fs::read(&args[0])?;
    key.verify(&artifact, &signature, true)?;
    let expected = format!("version:{}", args[2]);
    if !signature
        .trusted_comment()
        .split('\t')
        .any(|field| field == expected)
    {
        return Err("signed version differs from the announced version".into());
    }
    if artifact.is_empty() {
        return Err("empty artifact".into());
    }
    artifact[0] ^= 1;
    if key.verify(&artifact, &signature, true).is_ok() {
        return Err("tampered package was accepted".into());
    }
    println!("signature, signed version and tamper rejection PASS");
    Ok(())
}
