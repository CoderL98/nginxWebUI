//! Masking and filtering for DNS tokens and other secret fields.

use serde_json::Value;

pub const SECRET_SETTING_KEYS: &[&str] = &["CF_Token", "CF_Account_ID", "GCORE_Key"];
pub const SECRET_CERT_KEYS: &[&str] = &["cfToken", "cfAccountId", "gcoreKey", "GCORE_Key"];

pub fn is_secret_setting(key: &str) -> bool {
    SECRET_SETTING_KEYS.contains(&key)
}

pub fn is_placeholder_secret(value: &str) -> bool {
    let t = value.trim();
    if t.is_empty() {
        return true;
    }
    if t.chars().all(|c| c == '*' || c == '•' || c == '·') {
        return true;
    }
    t == "[set]" || t == "__unchanged__"
}

pub fn should_store_secret(value: &str) -> bool {
    !is_placeholder_secret(value)
}

pub fn setting_view(key: &str, value: &str) -> Value {
    if is_secret_setting(key) {
        serde_json::json!({
            "key": key,
            "value": "",
            "set": !value.is_empty(),
        })
    } else {
        serde_json::json!({
            "key": key,
            "value": value,
            "set": !value.is_empty(),
        })
    }
}

pub fn strip_object_secrets(value: &mut Value, keys: &[&str]) {
    if let Value::Object(map) = value {
        for key in keys {
            map.remove(*key);
        }
    }
}

pub fn mask_settings_in_export(tables: &mut serde_json::Map<String, Value>) {
    let Some(Value::Array(rows)) = tables.get_mut("setting") else {
        return;
    };
    for row in rows {
        let key = crate::store::str_of(row, "key");
        if is_secret_setting(&key) {
            if let Value::Object(map) = row {
                let set = map
                    .get("value")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
                map.insert("value".into(), "".into());
                map.insert("set".into(), set.into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn setting_get_masks_secrets() {
        let view = setting_view("CF_Token", "super-secret");
        assert_eq!(view["value"], "");
        assert_eq!(view["set"], true);
        let plain = setting_view("nginxPath", "/etc/nginx/nginx.conf");
        assert_eq!(plain["value"], "/etc/nginx/nginx.conf");
    }

    #[test]
    fn empty_and_bullet_masks_are_placeholders() {
        assert!(is_placeholder_secret(""));
        assert!(is_placeholder_secret("   "));
        assert!(is_placeholder_secret("********"));
        assert!(is_placeholder_secret("••••"));
        assert!(!should_store_secret("********"));
        assert!(should_store_secret("new-token-value"));
    }

    #[test]
    fn export_strips_setting_secret_values() {
        let mut tables = serde_json::Map::new();
        tables.insert(
            "setting".into(),
            json!([
                {"id": "1", "key": "CF_Token", "value": "abc"},
                {"id": "2", "key": "nginxPath", "value": "/etc/nginx/nginx.conf"}
            ]),
        );
        mask_settings_in_export(&mut tables);
        let rows = tables["setting"].as_array().unwrap();
        assert_eq!(rows[0]["value"], "");
        assert_eq!(rows[0]["set"], true);
        assert_eq!(rows[1]["value"], "/etc/nginx/nginx.conf");
    }

    #[test]
    fn cert_secret_fields_are_removed() {
        let mut row = json!({"domain": "a.com", "cfToken": "secret", "kind": "dns_cf"});
        strip_object_secrets(&mut row, SECRET_CERT_KEYS);
        assert!(row.get("cfToken").is_none());
        assert_eq!(row["domain"], "a.com");
    }
}
