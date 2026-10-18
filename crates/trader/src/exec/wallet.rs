//! Private key loading. The key is written by the owner into a file (hex, 32 bytes) and never leaves memory:
//! it is not logged, not copied, and errors never include it.
use std::path::Path;
use std::str::FromStr;

use alloy_signer_local::PrivateKeySigner;

/// Load the signer from `path`. On unix the file must not be readable by group or others (chmod 600).
pub fn load_signer(path: &Path) -> Result<PrivateKeySigner, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("key file {}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("key file {} is not a regular file", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(format!("key file {} is readable by other users (mode {:o}); run: chmod 600 {}", path.display(), mode & 0o777, path.display()));
        }
    }
    let raw = std::fs::read_to_string(path).map_err(|e| format!("key file {}: {e}", path.display()))?;
    parse_key(raw.trim()).map_err(|e| format!("key file {}: {e}", path.display()))
}

/// Parse a hex private key (with or without 0x). Error messages never echo the input.
pub fn parse_key(s: &str) -> Result<PrivateKeySigner, String> {
    let hex = s.strip_prefix("0x").unwrap_or(s);
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("must contain exactly one 32-byte private key in hex (64 hex characters)".into());
    }
    PrivateKeySigner::from_str(hex).map_err(|_| "not a valid secp256k1 private key".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Well-known test key (Hardhat/Anvil account #0). Never fund it.
    const TEST_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_ADDR: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

    #[test]
    fn parses_key_and_derives_address() {
        let s = parse_key(TEST_KEY).unwrap();
        assert_eq!(format!("{:#x}", s.address()), TEST_ADDR);
        assert_eq!(format!("{:#x}", parse_key(&TEST_KEY[2..]).unwrap().address()), TEST_ADDR);
    }

    #[test]
    fn rejects_malformed_keys_without_echoing_them() {
        let zz = "zz".repeat(32);
        let zeros = "0".repeat(64);
        for bad in ["", "0x1234", zz.as_str(), zeros.as_str()] {
            let e = parse_key(bad).unwrap_err();
            assert!(!e.contains("1234") && !e.contains("zz"), "error leaked input: {e}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_world_readable_key_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rhc_wallet_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("key");
        std::fs::write(&f, TEST_KEY).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_signer(&f).is_err());
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(format!("{:#x}", load_signer(&f).unwrap().address()), TEST_ADDR);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
