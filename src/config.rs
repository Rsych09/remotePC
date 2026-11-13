use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_autostart() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bot_token: String,
    pub allowed_user_id: u64,
    #[serde(default = "default_autostart")]
    pub autostart: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bot_token: "ВАШ_ТОКЕН_БОТА".to_string(),
            allowed_user_id: 0,
            autostart: true,
        }
    }
}

pub fn get_config_path() -> PathBuf {
    // 1. Сначала проверяем наличие config.toml рядом с .exe
    if let Ok(mut exe) = std::env::current_exe() {
        exe.pop();
        let exe_config = exe.join("config.toml");
        if exe_config.exists() {
            return exe_config;
        }
    }

    // 2. Затем проверяем наличие config.toml в текущей рабочей директории
    let cwd_config = PathBuf::from("config.toml");
    if cwd_config.exists() {
        return cwd_config;
    }

    // 3. Если файла нигде нет, путь по умолчанию — рядом с .exe
    if let Ok(mut exe) = std::env::current_exe() {
        exe.pop();
        exe.push("config.toml");
        return exe;
    }

    cwd_config
}

pub fn load_or_create_config() -> Result<Config, String> {
    let path = get_config_path();

    if !path.exists() {
        let template = r#"# Telegram Remote PC Control Agent Configuration
# Получите bot_token у @BotFather в Telegram
bot_token = "ВАШ_ТОКЕН_БОТА"

# Ваш числовой Telegram User ID (можно узнать у @userinfobot)
allowed_user_id = 0

# Автозагрузка программы при входе в Windows (true / false)
autostart = true
"#;
        if let Err(e) = std::fs::write(&path, template) {
            return Err(format!("Не удалось создать файл {:?}: {}", path, e));
        }

        return Err(format!(
            "Файл конфигурации создан по пути:\n{:?}\n\nПожалуйста, укажите в нем действительные bot_token и allowed_user_id перед запуском.",
            path
        ));
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Err(format!("Ошибка чтения {:?}: {}", path, e)),
    };

    let config: Config = match toml::from_str(&content) {
        Ok(cfg) => cfg,
        Err(e) => return Err(format!("Ошибка синтаксиса в {:?}: {}", path, e)),
    };

    if config.bot_token == "ВАШ_ТОКЕН_БОТА"
        || config.bot_token.trim().is_empty()
        || config.allowed_user_id == 0
    {
        return Err(format!(
            "Файл конфигурации {:?} содержит шаблонные значения.\nПожалуйста, укажите реальные bot_token и allowed_user_id.",
            path
        ));
    }

    Ok(config)
}

pub fn save_config(config: &Config) -> Result<(), String> {
    let path = get_config_path();
    let toml_str = match toml::to_string_pretty(config) {
        Ok(s) => s,
        Err(e) => return Err(format!("Ошибка сериализации config.toml: {}", e)),
    };

    if let Err(e) = std::fs::write(&path, toml_str) {
        return Err(format!("Не удалось записать в {:?}: {}", path, e));
    }

    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedState {
    pub last_panel_msg_id: Option<i32>,
    pub last_photo_msg_id: Option<i32>,
}

pub fn get_state_path() -> PathBuf {
    let mut path = get_config_path();
    path.set_file_name(".state.toml");
    path
}

pub fn load_saved_state() -> SavedState {
    let path = get_state_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        toml::from_str(&content).unwrap_or_default()
    } else {
        SavedState::default()
    }
}

pub fn save_saved_state(state: &SavedState) {
    let path = get_state_path();
    if let Ok(content) = toml::to_string(state) {
        let _ = std::fs::write(&path, content);
    }
}

pub fn show_native_message_box(title: &str, message: &str) {
    use std::os::windows::ffi::OsStrExt;

    let title_w: Vec<u16> = std::ffi::OsStr::new(title)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let msg_w: Vec<u16> = std::ffi::OsStr::new(message)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            msg_w.as_ptr(),
            title_w.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_deserialize_valid() {
        let toml_str = r#"
            bot_token = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
            allowed_user_id = 987654321
            autostart = true
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.bot_token, "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11");
        assert_eq!(config.allowed_user_id, 987654321);
        assert!(config.autostart);
    }

    #[test]
    fn test_config_deserialize_default_autostart() {
        let toml_str = r#"
            bot_token = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
            allowed_user_id = 987654321
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.autostart);
    }

    #[test]
    fn test_config_template_detection() {
        let default_cfg = Config::default();
        assert_eq!(default_cfg.bot_token, "ВАШ_ТОКЕН_БОТА");
        assert_eq!(default_cfg.allowed_user_id, 0);
        assert!(default_cfg.autostart);
    }
}
