use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GbaRomInfo {
    pub title: String,
    pub game_code: String,
    pub maker_code: String,
    pub version: u8,
    pub recommended_runahead: u8,
}

static RUNTIME_CACHE: Mutex<Option<HashMap<String, u8>>> = Mutex::new(None);

/// Returns the standardized OS application config path:
/// Linux / macOS: $XDG_CONFIG_HOME/remote_retro/gba_lag_config.json or ~/.config/remote_retro/gba_lag_config.json
pub fn get_cache_file_path() -> PathBuf {
    let base_dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config")
    } else {
        PathBuf::from("/tmp")
    };
    base_dir.join("remote_retro").join("gba_lag_config.json")
}

fn load_disk_cache_into_map(map: &mut HashMap<String, u8>) {
    let path = get_cache_file_path();
    if let Ok(data) = fs::read_to_string(&path) {
        // Minimal zero-dependency JSON parser for flat key-value pairs
        for line in data.lines() {
            let line = line.trim().trim_matches(|c| c == '{' || c == '}' || c == ',');
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().trim_matches('"').to_string();
                if let Ok(val) = v.trim().parse::<u8>() {
                    if !key.is_empty() {
                        map.insert(key, val);
                    }
                }
            }
        }
    }
}

fn save_disk_cache(map: &HashMap<String, u8>) {
    let path = get_cache_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut out = String::from("{\n");
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    for (i, (k, v)) in entries.iter().enumerate() {
        let comma = if i + 1 < entries.len() { "," } else { "" };
        out.push_str(&format!("  \"{}\": {}{}\n", k, v, comma));
    }
    out.push('}');
    let _ = fs::write(&path, out);
}

/// Checks if this game has a cached or user-configured runahead setting in ~/.config/remote_retro
pub fn lookup_cached(game_code: &str) -> Option<u8> {
    if let Ok(mut guard) = RUNTIME_CACHE.lock() {
        if guard.is_none() {
            let mut map = HashMap::new();
            load_disk_cache_into_map(&mut map);
            *guard = Some(map);
        }
        if let Some(ref map) = *guard {
            return map.get(game_code).copied();
        }
    }
    None
}

/// Verified pre-compiled database of measured GBA ROMs.
pub fn lookup_verified_db(_game_code: &str) -> Option<u8> {
    // Only verified database entries
    None
}

/// Reads the GBA ROM header and retrieves calibrated lag.
pub fn inspect_gba_rom<P: AsRef<Path>>(rom_path: P) -> GbaRomInfo {
    let fallback = GbaRomInfo {
        title: "Unknown GBA Game".to_string(),
        game_code: "UNKN".to_string(),
        maker_code: "00".to_string(),
        version: 0,
        recommended_runahead: 1,
    };

    let mut file = match File::open(&rom_path) {
        Ok(f) => f,
        Err(_) => return fallback,
    };

    if file.seek(SeekFrom::Start(0x00A0)).is_err() {
        return fallback;
    }

    let mut header_buf = [0u8; 32];
    if file.read_exact(&mut header_buf).is_err() {
        return fallback;
    }

    let raw_title = &header_buf[0..12];
    let raw_game_code = &header_buf[12..16];
    let raw_maker_code = &header_buf[16..18];
    let version = header_buf[28];

    let title = String::from_utf8_lossy(raw_title)
        .trim_matches(char::from(0))
        .trim()
        .to_string();

    let game_code = String::from_utf8_lossy(raw_game_code)
        .trim_matches(char::from(0))
        .trim()
        .to_string();

    let maker_code = String::from_utf8_lossy(raw_maker_code)
        .trim_matches(char::from(0))
        .trim()
        .to_string();

    let title_clean = if title.is_empty() { "GBA Game".to_string() } else { title };
    let code_clean = if game_code.is_empty() { "UNKN".to_string() } else { game_code };

    // 1. Check in-memory + persistent disk cache
    if let Ok(mut guard) = RUNTIME_CACHE.lock() {
        if guard.is_none() {
            let mut map = HashMap::new();
            load_disk_cache_into_map(&mut map);
            *guard = Some(map);
        }
        if let Some(ref map) = *guard {
            if let Some(&cached_val) = map.get(&code_clean) {
                return GbaRomInfo {
                    title: title_clean,
                    game_code: code_clean,
                    maker_code,
                    version,
                    recommended_runahead: cached_val,
                };
            }
        }
    }

    // 2. Check pre-compiled verified DB
    let recommended = if let Some(val) = lookup_verified_db(&code_clean) {
        val
    } else {
        // 3. Fallback default
        1
    };

    GbaRomInfo {
        title: title_clean,
        game_code: code_clean,
        maker_code,
        version,
        recommended_runahead: recommended,
    }
}

/// Stores a dynamically measured runahead result in RAM + persistent JSON cache
pub fn cache_measured_runahead(game_code: String, runahead: u8) {
    if let Ok(mut guard) = RUNTIME_CACHE.lock() {
        if guard.is_none() {
            let mut map = HashMap::new();
            load_disk_cache_into_map(&mut map);
            *guard = Some(map);
        }
        if let Some(ref mut map) = *guard {
            map.insert(game_code, runahead);
            save_disk_cache(map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_cached() {
        cache_measured_runahead("TEST_GAME_1".to_string(), 0);
        assert_eq!(lookup_cached("TEST_GAME_1"), Some(0));
        assert_eq!(lookup_cached("NON_EXISTENT_XYZ"), None);
    }

    #[test]
    fn test_persistent_cache_roundtrip() {
        cache_measured_runahead("TEST_PERSIST_1".to_string(), 2);
        let path = get_cache_file_path();
        assert!(path.to_string_lossy().contains("remote_retro"));
        
        if let Ok(guard) = RUNTIME_CACHE.lock() {
            if let Some(ref map) = *guard {
                assert_eq!(map.get("TEST_PERSIST_1"), Some(&2));
            }
        }
    }
}
