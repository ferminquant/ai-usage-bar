//! Per-user Windows startup registration used by the desktop shell.
//!
//! The installer and the shell intentionally share the same `HKCU\Run`
//! value. The shell never removes a value that points at another executable,
//! so a stale or conflicting registration cannot be deleted accidentally.

use std::path::Path;

pub const STARTUP_VALUE_NAME: &str = "AI Usage Bar";

/// Registry value used to remember the user's startup preference separately
/// from the executable command. Windows or another cleanup tool can remove a
/// `Run` value; keeping the preference in our own key lets the installer
/// restore that command without silently re-enabling a user who deliberately
/// turned startup off.
pub const STARTUP_PREFERENCE_VALUE_NAME: &str = "StartupEnabled";

/// Extract the executable path from a Windows `Run` command value.
///
/// AI Usage Bar writes a quoted path with no arguments. The small amount of
/// unquoted parsing here also lets us recognize older or manually edited
/// values without treating an unrelated command's arguments as its path.
pub fn startup_command_path(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(quoted) = value.strip_prefix('"') {
        return quoted.split_once('"').map(|(path, _)| path);
    }
    value.split_whitespace().next()
}

/// Return whether a startup command points at the supplied executable.
pub fn startup_value_matches_executable(value: &str, executable: &Path) -> bool {
    let Some(startup_path) = startup_command_path(value) else {
        return false;
    };
    startup_path.eq_ignore_ascii_case(executable.to_string_lossy().as_ref())
}

/// Interpret a `REG_DWORD` startup-preference value.
///
/// Values outside {0, 1} are not values this app wrote and are treated as "no
/// preference recorded" so a single corrupted DWORD cannot wedge the toggle.
/// This mirrors `Get-StartupPreference` in the installer.
#[cfg(windows)]
fn dword_to_startup_preference(value: u32) -> Option<bool> {
    match value {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

#[cfg(windows)]
mod windows_registry {
    use super::{startup_value_matches_executable, STARTUP_VALUE_NAME};
    use std::fmt;
    use std::path::{Path, PathBuf};

    use windows::core::w;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_EXPAND_SZ,
        REG_NONE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    const PREFERENCE_KEY: windows::core::PCWSTR = w!("Software\\AI Usage Bar");

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum StartupError {
        CurrentExecutable,
        Registry(u32),
        InvalidValueType,
        InvalidValue,
        OwnedByAnotherExecutable,
    }

    impl fmt::Display for StartupError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::CurrentExecutable => {
                    write!(formatter, "could not determine the current executable")
                }
                Self::Registry(code) => write!(formatter, "Windows registry error {code}"),
                Self::InvalidValueType => {
                    write!(formatter, "startup registry value has an invalid type")
                }
                Self::InvalidValue => write!(formatter, "startup registry value is malformed"),
                Self::OwnedByAnotherExecutable => write!(
                    formatter,
                    "startup entry is owned by another executable; reinstall or repair it first"
                ),
            }
        }
    }

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    fn registry_error(code: windows::Win32::Foundation::WIN32_ERROR) -> StartupError {
        StartupError::Registry(code.0)
    }

    fn open_run_key(
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<RegistryKey, StartupError> {
        let mut key = HKEY::default();
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key) };
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        Ok(RegistryKey(key))
    }

    fn create_run_key(
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<RegistryKey, StartupError> {
        let mut key = HKEY::default();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                None,
                w!(""),
                REG_OPTION_NON_VOLATILE,
                access,
                None,
                &mut key,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        Ok(RegistryKey(key))
    }

    fn read_startup_value() -> Result<Option<String>, StartupError> {
        let key = match open_run_key(KEY_READ) {
            Ok(key) => key,
            Err(StartupError::Registry(code)) if code == ERROR_FILE_NOT_FOUND.0 => return Ok(None),
            Err(error) => return Err(error),
        };
        // Keep the UTF-16 value name alive for every registry call below.
        let value_name_units: Vec<u16> = STARTUP_VALUE_NAME.encode_utf16().chain(Some(0)).collect();
        let value_name = windows::core::PCWSTR::from_raw(value_name_units.as_ptr());
        let mut value_type = REG_NONE;
        let mut byte_count = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                value_name,
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_count),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
            return Err(StartupError::InvalidValueType);
        }
        if byte_count == 0 {
            return Err(StartupError::InvalidValue);
        }

        let mut value_units = vec![0u16; (byte_count as usize).div_ceil(2)];
        let mut actual_byte_count = byte_count;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                value_name,
                None,
                Some(&mut value_type),
                Some(value_units.as_mut_ptr().cast()),
                Some(&mut actual_byte_count),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        value_units.truncate((actual_byte_count as usize) / 2);
        if let Some(end) = value_units.iter().position(|unit| *unit == 0) {
            value_units.truncate(end);
        }
        let value = String::from_utf16(&value_units).map_err(|_| StartupError::InvalidValue)?;
        if value.trim().is_empty() {
            return Err(StartupError::InvalidValue);
        }
        Ok(Some(value))
    }

    fn current_executable() -> Result<PathBuf, StartupError> {
        std::env::current_exe().map_err(|_| StartupError::CurrentExecutable)
    }

    fn command_for(executable: &Path) -> Result<Vec<u8>, StartupError> {
        let path = executable.to_str().ok_or(StartupError::CurrentExecutable)?;
        let command: Vec<u16> = format!("\"{path}\"")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let bytes =
            unsafe { std::slice::from_raw_parts(command.as_ptr().cast::<u8>(), command.len() * 2) };
        Ok(bytes.to_vec())
    }

    fn value_name() -> Vec<u16> {
        STARTUP_VALUE_NAME.encode_utf16().chain(Some(0)).collect()
    }

    fn preference_value_name() -> Vec<u16> {
        super::STARTUP_PREFERENCE_VALUE_NAME
            .encode_utf16()
            .chain(Some(0))
            .collect()
    }

    fn read_startup_preference() -> Result<Option<bool>, StartupError> {
        let key = match open_preference_key(KEY_READ) {
            Ok(key) => key,
            Err(StartupError::Registry(code)) if code == ERROR_FILE_NOT_FOUND.0 => return Ok(None),
            Err(error) => return Err(error),
        };
        let value_name_units = preference_value_name();
        let value_name = windows::core::PCWSTR::from_raw(value_name_units.as_ptr());
        let mut value_type = REG_NONE;
        let mut byte_count = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                value_name,
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_count),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        if value_type != REG_DWORD || byte_count != std::mem::size_of::<u32>() as u32 {
            return Err(StartupError::InvalidValueType);
        }

        let mut value = 0u32;
        let mut actual_byte_count = byte_count;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                value_name,
                None,
                Some(&mut value_type),
                Some((&mut value as *mut u32).cast()),
                Some(&mut actual_byte_count),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        if actual_byte_count != std::mem::size_of::<u32>() as u32 {
            return Err(StartupError::InvalidValue);
        }
        Ok(super::dword_to_startup_preference(value))
    }

    fn open_preference_key(
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<RegistryKey, StartupError> {
        let mut key = HKEY::default();
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PREFERENCE_KEY, None, access, &mut key) };
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        Ok(RegistryKey(key))
    }

    fn create_preference_key(
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<RegistryKey, StartupError> {
        let mut key = HKEY::default();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PREFERENCE_KEY,
                None,
                w!(""),
                REG_OPTION_NON_VOLATILE,
                access,
                None,
                &mut key,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        Ok(RegistryKey(key))
    }

    fn write_startup_preference(enabled: bool) -> Result<(), StartupError> {
        let key = create_preference_key(KEY_SET_VALUE)?;
        let value_name_units = preference_value_name();
        let value_name = windows::core::PCWSTR::from_raw(value_name_units.as_ptr());
        // REG_DWORD is little-endian per the registry contract, independent of
        // host endianness. Use an explicit little-endian encoding.
        let value = u32::from(enabled).to_le_bytes();
        let status = unsafe { RegSetValueExW(key.0, value_name, None, REG_DWORD, Some(&value)) };
        if status != ERROR_SUCCESS {
            return Err(registry_error(status));
        }
        Ok(())
    }

    fn delete_startup_preference() -> Result<(), StartupError> {
        let key = match open_preference_key(KEY_SET_VALUE) {
            Ok(key) => key,
            Err(StartupError::Registry(code)) if code == ERROR_FILE_NOT_FOUND.0 => return Ok(()),
            Err(error) => return Err(error),
        };
        let value_name_units = preference_value_name();
        let value_name = windows::core::PCWSTR::from_raw(value_name_units.as_ptr());
        let status = unsafe { RegDeleteValueW(key.0, value_name) };
        if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
            return Err(registry_error(status));
        }
        Ok(())
    }

    fn restore_startup_preference(previous: Option<bool>) {
        match previous {
            Some(enabled) => {
                let _ = write_startup_preference(enabled);
            }
            None => {
                let _ = delete_startup_preference();
            }
        }
    }

    pub fn auto_start_enabled() -> Result<bool, StartupError> {
        let Some(value) = read_startup_value()? else {
            return Ok(false);
        };
        Ok(startup_value_matches_executable(
            &value,
            &current_executable()?,
        ))
    }

    pub fn set_auto_start_enabled(enabled: bool) -> Result<(), StartupError> {
        let executable = current_executable()?;
        let existing = read_startup_value()?;
        let previous_preference = read_startup_preference()?;
        let value_name_units = value_name();
        let value_name = windows::core::PCWSTR::from_raw(value_name_units.as_ptr());

        if let Some(existing) = existing.as_ref() {
            if !startup_value_matches_executable(existing, &executable) {
                return Err(StartupError::OwnedByAnotherExecutable);
            }
        }

        // Update the preference first. If the Run-key operation fails, put the
        // previous preference back so the two records cannot drift apart.
        write_startup_preference(enabled)?;

        if !enabled {
            if existing.is_none() {
                return Ok(());
            }
            let key = match open_run_key(KEY_SET_VALUE) {
                Ok(key) => key,
                Err(error) => {
                    restore_startup_preference(previous_preference);
                    return Err(error);
                }
            };
            let status = unsafe { RegDeleteValueW(key.0, value_name) };
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                restore_startup_preference(previous_preference);
                return Err(registry_error(status));
            }
            return Ok(());
        }

        let key = match create_run_key(KEY_SET_VALUE) {
            Ok(key) => key,
            Err(error) => {
                restore_startup_preference(previous_preference);
                return Err(error);
            }
        };
        let command = match command_for(&executable) {
            Ok(command) => command,
            Err(error) => {
                restore_startup_preference(previous_preference);
                return Err(error);
            }
        };
        let status = unsafe { RegSetValueExW(key.0, value_name, None, REG_SZ, Some(&command)) };
        if status != ERROR_SUCCESS {
            restore_startup_preference(previous_preference);
            return Err(registry_error(status));
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use windows_registry::{auto_start_enabled, set_auto_start_enabled, StartupError};

#[cfg(test)]
mod tests {
    use super::{startup_command_path, startup_value_matches_executable};
    use std::path::Path;

    #[cfg(windows)]
    use super::dword_to_startup_preference;

    #[test]
    fn parses_quoted_and_unquoted_run_commands() {
        assert_eq!(
            startup_command_path(r#""C:\Program Files\AI Usage Bar\ai-usage-bar-shell.exe""#),
            Some(r#"C:\Program Files\AI Usage Bar\ai-usage-bar-shell.exe"#),
        );
        assert_eq!(
            startup_command_path(r#"C:\Tools\bar.exe --hidden"#),
            Some(r#"C:\Tools\bar.exe"#),
        );
        assert_eq!(startup_command_path("  "), None);
    }

    #[test]
    fn executable_matching_is_case_insensitive_and_does_not_match_arguments() {
        let executable = Path::new(r#"C:\Program Files\AI Usage Bar\ai-usage-bar-shell.exe"#);
        assert!(startup_value_matches_executable(
            r#""c:\program files\ai usage bar\AI-USAGE-BAR-SHELL.EXE" --hidden"#,
            executable,
        ));
        assert!(!startup_value_matches_executable(
            r#""C:\Other\ai-usage-bar-shell.exe""#,
            executable,
        ));
    }

    #[test]
    #[cfg(windows)]
    fn startup_preference_dword_mapping() {
        assert_eq!(dword_to_startup_preference(0), Some(false));
        assert_eq!(dword_to_startup_preference(1), Some(true));
        // Anything outside {0, 1} means "no preference recorded", matching
        // Get-StartupPreference in the installer, so a corrupted DWORD cannot
        // wedge the startup toggle.
        assert_eq!(dword_to_startup_preference(2), None);
        assert_eq!(dword_to_startup_preference(u32::MAX), None);
    }
}
