//! Самоограничение процесса Job Object'ом для разбора недоверенных демок.
//!
//! Дочерний процесс (`cs2-cli inspect`) в начале `main` помещает сам себя в job с лимитом памяти
//! и запретом порождать процессы. Handle job не закрывается до конца процесса: флага
//! `KILL_ON_JOB_CLOSE` здесь нет, иначе закрытие последнего handle убило бы сам процесс.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use super::last_error;
use crate::HostError;

/// Job Object, в который процесс поместил сам себя. Handle живёт до конца процесса.
pub struct Guard {
    handle: HANDLE,
}

// SAFETY: handle job — просто идентификатор ядра; Win32-функции, которые мы к нему применяем,
// потокобезопасны по документации.
unsafe impl Send for Guard {}
// SAFETY: см. выше — все вызовы по handle допускают одновременное использование из разных потоков.
unsafe impl Sync for Guard {}

/// Ограничивает память текущего процесса значением `bytes` и запрещает ему порождать процессы.
pub fn limit_self_memory(bytes: u64) -> Result<Guard, HostError> {
    // SAFETY: оба аргумента — null (безымянный job с атрибутами по умолчанию); результат проверяем на null.
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(HostError::JobAssign(format!(
            "CreateJobObjectW: {}",
            last_error()
        )));
    }
    // SAFETY: структура состоит из целых чисел и указателей, нулевое значение для неё корректно.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    info.BasicLimitInformation.ActiveProcessLimit = 1;
    info.ProcessMemoryLimit = bytes as usize;
    // SAFETY: handle только что создан и валиден; указатель и длина описывают живую структуру `info`.
    let ok = unsafe {
        SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        let e = last_error();
        // SAFETY: handle наш и ещё не закрыт; после этого значение не используется.
        unsafe { CloseHandle(handle) };
        return Err(HostError::JobAssign(format!(
            "SetInformationJobObject: {e}"
        )));
    }
    // SAFETY: псевдо-handle текущего процесса всегда валиден; handle job создан выше.
    let ok = unsafe { AssignProcessToJobObject(handle, GetCurrentProcess()) };
    if ok == 0 {
        let e = last_error();
        // SAFETY: handle наш и ещё не закрыт; после этого значение не используется.
        unsafe { CloseHandle(handle) };
        return Err(HostError::JobAssign(format!(
            "AssignProcessToJobObject: {e}"
        )));
    }
    Ok(Guard { handle })
}

impl Guard {
    /// Пиковая память процесса за всё время жизни (`PeakProcessMemoryUsed`); `None`, если запрос не удался.
    pub fn peak_bytes(&self) -> Option<u64> {
        // SAFETY: структура состоит из целых чисел и указателей, нулевое значение для неё корректно.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: handle валиден всё время жизни `Guard`; указатель и длина описывают живую структуру `info`.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(info.PeakProcessMemoryUsed as u64)
    }
}

/// Подавляет диалоги Windows об ошибках (`SEM_FAILCRITICALERRORS`, `SEM_NOGPFAULTERRORBOX`), чтобы
/// аварийно умерший разбор не повис на окне, которое некому закрыть.
pub fn quiet_crash_dialogs() {
    // SAFETY: SetErrorMode принимает набор флагов и не работает с указателями; побочный эффект —
    // режим ошибок текущего процесса.
    unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
}
