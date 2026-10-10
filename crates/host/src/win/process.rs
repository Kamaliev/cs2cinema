//! Процессы по handle: живость без угадываний и снимок списка процессов.
//!
//! Живость определяется ожиданием на handle: «не знаю» (любая ошибка) — это `Unknown`, а не `Alive`.

use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};

use super::job::Job;
use super::last_error;
use crate::{HostError, Liveness};

/// Процесс, открытый по PID: владеет handle и закрывает его в `Drop`.
pub struct OwnedProc {
    pid: u32,
    handle: HANDLE,
}

// SAFETY: handle процесса — идентификатор ядра; Wait/GetExitCode/Query по нему допустимы из любого потока.
unsafe impl Send for OwnedProc {}

impl OwnedProc {
    /// Открывает процесс для ожидания и чтения сведений (завершать его этот handle не умеет).
    pub fn open(pid: u32) -> Result<Self, HostError> {
        // SAFETY: аргументы — права доступа, флаг наследования и PID; результат проверяем на null.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return Err(HostError::Io(format!(
                "OpenProcess({pid}): {}",
                last_error()
            )));
        }
        Ok(OwnedProc { pid, handle })
    }

    /// Открывает процесс ещё и с правом завершения: для режима «игра принята без job», когда
    /// завершить её можно только своим handle (D-02, запасной путь).
    pub fn open_owned(pid: u32) -> Result<Self, HostError> {
        // SAFETY: аргументы — права доступа, флаг наследования и PID; результат проверяем на null.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return Err(HostError::Io(format!(
                "OpenProcess({pid}, с правом завершения): {}",
                last_error()
            )));
        }
        Ok(OwnedProc { pid, handle })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Завершает процесс своим handle (нужен `open_owned`; у `open` права нет и вернётся ошибка ОС).
    pub fn kill(&self, code: u32) -> Result<(), HostError> {
        // SAFETY: handle открыт нами и жив до конца `self`.
        let ok = unsafe { TerminateProcess(self.handle, code) };
        if ok == 0 {
            return Err(HostError::Io(format!(
                "TerminateProcess({}): {}",
                self.pid,
                last_error()
            )));
        }
        Ok(())
    }

    /// Жив ли процесс: ожидание на handle без задержки.
    pub fn liveness(&self) -> Liveness {
        self.wait(Duration::ZERO)
    }

    /// Ждёт завершения процесса не дольше `timeout`.
    /// `WAIT_TIMEOUT` — жив, `WAIT_OBJECT_0` — завершён (с кодом выхода), всё остальное — `Unknown`.
    pub fn wait(&self, timeout: Duration) -> Liveness {
        // u32::MAX - «бесконечно»; до него не доходим
        let ms = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: handle открыт нами и жив до конца `self`.
        let r = unsafe { WaitForSingleObject(self.handle, ms) };
        match r {
            WAIT_TIMEOUT => Liveness::Alive,
            WAIT_OBJECT_0 => {
                let mut code = 0u32;
                // SAFETY: handle валиден, `code` — живая переменная для результата.
                let ok = unsafe { GetExitCodeProcess(self.handle, &mut code) };
                Liveness::Exited((ok != 0).then_some(code))
            }
            WAIT_FAILED => Liveness::Unknown(format!(
                "WaitForSingleObject({}): {}",
                self.pid,
                last_error()
            )),
            other => Liveness::Unknown(format!(
                "WaitForSingleObject({}) вернул {other:#x}",
                self.pid
            )),
        }
    }

    /// Полный путь к exe процесса.
    pub fn image_path(&self) -> Option<PathBuf> {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` и `len` живы, `len` — вместимость буфера в символах; handle валиден.
        let ok = unsafe {
            QueryFullProcessImageNameW(self.handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
        };
        (ok != 0).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len as usize])))
    }

    /// Время создания процесса в 100-нс единицах FILETIME.
    pub fn creation_time(&self) -> Option<u64> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: все четыре указателя ведут на живые переменные; handle валиден.
        let ok = unsafe {
            GetProcessTimes(
                self.handle,
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        (ok != 0)
            .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    /// Находится ли процесс в этом job (`None` — не удалось узнать).
    pub fn in_job(&self, job: &Job) -> Option<bool> {
        let mut result = 0;
        // SAFETY: оба handle валидны, `result` — живая переменная.
        let ok = unsafe { IsProcessInJob(self.handle, job.raw(), &mut result) };
        (ok != 0).then_some(result != 0)
    }
}

impl Drop for OwnedProc {
    fn drop(&mut self) {
        // SAFETY: handle открыт в `open` и закрывается ровно один раз.
        unsafe { CloseHandle(self.handle) };
    }
}

/// Живость процесса по PID. Если процесс не открылся — `Unknown` с текстом ошибки, никогда не `Alive`.
pub fn liveness_of_pid(pid: u32) -> Liveness {
    match OwnedProc::open(pid) {
        Ok(p) => p.liveness(),
        Err(e) => Liveness::Unknown(e.to_string()),
    }
}

/// Запись из снимка процессов.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcEntry {
    pub pid: u32,
    pub parent_pid: u32,
    /// Имя образа, например `cs2.exe`.
    pub exe_name: String,
}

/// Снимок всех процессов системы (для предпроверок и запасного пути «принять cs2.exe по PID»).
pub fn snapshot_processes() -> Result<Vec<ProcEntry>, HostError> {
    // SAFETY: флаги и PID=0 (все процессы); результат сравниваем с INVALID_HANDLE_VALUE.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(HostError::Io(format!(
            "CreateToolhelp32Snapshot: {}",
            last_error()
        )));
    }
    let mut out = Vec::new();
    // SAFETY: структура из целых чисел и массива, ноль корректен; `dwSize` обязателен для API.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: handle снимка валиден, `entry` с заполненным `dwSize` жив.
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) };
    let first_err = if ok == 0 { Some(last_error()) } else { None };
    while ok != 0 {
        let end = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        out.push(ProcEntry {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            exe_name: String::from_utf16_lossy(&entry.szExeFile[..end]),
        });
        // SAFETY: те же условия, что и для Process32FirstW.
        ok = unsafe { Process32NextW(snap, &mut entry) };
    }
    // SAFETY: handle снимка открыт выше; закрываем в любом исходе ровно один раз.
    unsafe { CloseHandle(snap) };
    match first_err {
        Some(e) => Err(HostError::Io(format!("Process32FirstW: {e}"))),
        None => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liveness_of_a_missing_process_is_unknown_not_alive() {
        match liveness_of_pid(u32::MAX) {
            Liveness::Unknown(text) => {
                assert!(!text.is_empty(), "ошибка ОС должна попасть в текст")
            }
            other => panic!("ожидалось Unknown, получено {other:?}"),
        }
    }

    #[test]
    fn snapshot_finds_the_current_process() {
        let me = std::process::id();
        let all = snapshot_processes().unwrap();
        let entry = all
            .iter()
            .find(|e| e.pid == me)
            .expect("текущий процесс в снимке");
        assert!(!entry.exe_name.is_empty());
    }
}
