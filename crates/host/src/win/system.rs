//! Факты окружения Windows: свободное место на томе и версия файла (PRE-06, DIF-01).

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO,
    VerQueryValueW,
};

use super::last_error;
use crate::HostError;

/// Строка UTF-16 с завершающим нулём для Win32.
fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Байты, доступные вызывающему (`lpFreeBytesAvailableToCaller`, с учётом квот), на томе пути.
/// Несуществующий путь заменяется ближайшим существующим предком.
pub fn free_space(path: &Path) -> Result<u64, HostError> {
    let Some(existing) = path.ancestors().find(|p| p.exists()) else {
        return Err(HostError::Io(format!(
            "нет существующего каталога для {}",
            path.display()
        )));
    };
    let w = wide(existing);
    let mut avail = 0u64;
    let mut total = 0u64;
    let mut total_free = 0u64;
    // SAFETY: `w` — строка с нулём, три указателя ведут на живые переменные.
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut total_free) };
    if ok == 0 {
        return Err(HostError::Io(format!(
            "GetDiskFreeSpaceExW: {}",
            last_error()
        )));
    }
    Ok(avail)
}

/// Версия файла `a.b.c.d` из ресурса версии; любая ошибка — `None`.
pub fn file_version(path: &Path) -> Option<String> {
    let w = wide(path);
    let mut handle = 0u32;
    // SAFETY: `w` — строка с нулём, `handle` — живая переменная.
    let size = unsafe { GetFileVersionInfoSizeW(w.as_ptr(), &mut handle) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: буфер длиной `size` байт жив на время вызова.
    let ok = unsafe { GetFileVersionInfoW(w.as_ptr(), 0, size, buf.as_mut_ptr().cast()) };
    if ok == 0 {
        return None;
    }
    let root = [u16::from(b'\\'), 0];
    let mut info: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `buf` содержит корректный блок версии; `info` и `len` — живые переменные для результата.
    let ok = unsafe { VerQueryValueW(buf.as_ptr().cast(), root.as_ptr(), &mut info, &mut len) };
    if ok == 0 || info.is_null() || (len as usize) < size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    // SAFETY: `info` указывает внутрь `buf` (он жив) на VS_FIXEDFILEINFO длиной не меньше структуры;
    // читаем без выравнивания, так как внутри буфера u8 оно не гарантировано.
    let fixed = unsafe { std::ptr::read_unaligned(info.cast::<VS_FIXEDFILEINFO>()) };
    Some(format!(
        "{}.{}.{}.{}",
        fixed.dwFileVersionMS >> 16,
        fixed.dwFileVersionMS & 0xFFFF,
        fixed.dwFileVersionLS >> 16,
        fixed.dwFileVersionLS & 0xFFFF
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_of_the_temp_dir_is_positive() {
        let n = free_space(&std::env::temp_dir()).unwrap();
        assert!(n > 0, "на томе временной папки нет места?");
    }

    #[test]
    fn free_space_of_a_missing_subdir_falls_back_to_its_parent() {
        let parent = std::env::temp_dir();
        let missing = parent.join("cs2cinema-no-such-dir").join("a").join("b");
        let (p, m) = (free_space(&parent).unwrap(), free_space(&missing).unwrap());
        // место на томе могло чуть измениться между вызовами: допускаем 64 МиБ
        assert!(p.abs_diff(m) < 64 << 20, "{p} против {m}");
    }

    #[test]
    fn file_version_of_cmd_has_four_parts() {
        let root = std::env::var_os("SystemRoot").expect("SystemRoot");
        let cmd = Path::new(&root).join("System32").join("cmd.exe");
        let v = file_version(&cmd).expect("у cmd.exe есть ресурс версии");
        let parts: Vec<&str> = v.split('.').collect();
        assert_eq!(parts.len(), 4, "{v}");
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()), "{v}");
        assert!(file_version(Path::new(r"C:\no\such\file.exe")).is_none());
    }
}
