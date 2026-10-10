//! Job Object: владелец дерева процессов игры.
//!
//! Все процессы сессии (HLAE, `cs2.exe`, ffmpeg от HLAE) живут в одном job с флагом
//! `KILL_ON_JOB_CLOSE`: закрытие handle или `terminate` завершает всё дерево, а чужие процессы
//! (например, собственный `cs2.exe` пользователя) job не затрагивает — завершаем только своё.

use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE,
};

use super::last_error;
use crate::HostError;

/// Win32 `ERROR_MORE_DATA`: буфер для списка PID мал.
const ERROR_MORE_DATA: i32 = 234;
/// Потолок размера списка PID — защита от бесконечного удвоения.
const MAX_PIDS: usize = 1 << 16;

/// Безопасная обёртка Job Object. Закрытие (`Drop`) завершает все процессы внутри.
pub struct Job {
    handle: HANDLE,
}

// SAFETY: handle job — просто идентификатор ядра; функции Win32, которые мы к нему применяем
// (Assign/Query/Terminate/CloseHandle), потокобезопасны по документации.
unsafe impl Send for Job {}
// SAFETY: см. выше — все вызовы по handle допускают одновременное использование из разных потоков.
unsafe impl Sync for Job {}

impl Job {
    /// Создаёт job с `KILL_ON_JOB_CLOSE`.
    pub fn new() -> Result<Job, HostError> {
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
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
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
        Ok(Job { handle })
    }

    /// Сырой handle для `IsProcessInJob`.
    pub(crate) fn raw(&self) -> HANDLE {
        self.handle
    }

    /// Помещает только что запущенный дочерний процесс (и всех его будущих потомков) в job.
    pub fn assign_child(&self, child: &Child) -> Result<(), HostError> {
        // SAFETY: оба handle валидны на время вызова (job — наш, процесс — внутри живого `Child`).
        let ok = unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle() as HANDLE) };
        if ok == 0 {
            return Err(HostError::JobAssign(format!(
                "AssignProcessToJobObject: {}",
                last_error()
            )));
        }
        Ok(())
    }

    /// Помещает в job уже запущенный процесс по PID (запасной путь: игру запустил загрузчик HLAE).
    pub fn assign_pid(&self, pid: u32) -> Result<(), HostError> {
        // SAFETY: аргументы — права доступа, флаг наследования и PID; результат проверяем на null.
        let h = unsafe {
            OpenProcess(
                PROCESS_SET_QUOTA
                    | PROCESS_TERMINATE
                    | PROCESS_SYNCHRONIZE
                    | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if h.is_null() {
            return Err(HostError::JobAssign(format!(
                "OpenProcess({pid}): {}",
                last_error()
            )));
        }
        // SAFETY: job и только что открытый handle процесса валидны.
        let ok = unsafe { AssignProcessToJobObject(self.handle, h) };
        let err = if ok == 0 { Some(last_error()) } else { None };
        // SAFETY: handle открыт выше и закрывается ровно один раз.
        unsafe { CloseHandle(h) };
        match err {
            None => Ok(()),
            Some(e) => Err(HostError::JobAssign(format!(
                "AssignProcessToJobObject({pid}): {e}"
            ))),
        }
    }

    /// PID всех процессов в job.
    pub fn pids(&self) -> Result<Vec<u32>, HostError> {
        let mut cap = 16usize;
        loop {
            let bytes = size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() + cap * size_of::<usize>();
            // буфер из usize — нужное выравнивание для структуры с массивом usize
            let mut buf = vec![0usize; bytes.div_ceil(size_of::<usize>())];
            let list = buf.as_mut_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
            // SAFETY: `list` указывает на буфер длиной не меньше `bytes`, выровненный по usize; handle валиден.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicProcessIdList,
                    list.cast(),
                    bytes as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if code == ERROR_MORE_DATA && cap < MAX_PIDS {
                    cap *= 2;
                    continue;
                }
                return Err(HostError::Io(format!(
                    "QueryInformationJobObject(список PID): {}",
                    last_error()
                )));
            }
            // SAFETY: запрос успешен — заголовок заполнен; читаем поля через указатель на наш буфер.
            let (assigned, listed) = unsafe {
                (
                    (*list).NumberOfAssignedProcesses as usize,
                    (*list).NumberOfProcessIdsInList as usize,
                )
            };
            if assigned > listed && cap < MAX_PIDS {
                cap = (assigned * 2).max(cap * 2);
                continue;
            }
            // SAFETY: первые `listed` элементов массива лежат внутри буфера (listed <= cap); база — поле структуры.
            let ids = unsafe {
                let base = (&raw const (*list).ProcessIdList).cast::<usize>();
                std::slice::from_raw_parts(base, listed)
                    .iter()
                    .map(|p| *p as u32)
                    .collect()
            };
            return Ok(ids);
        }
    }

    /// Сколько процессов в job живо прямо сейчас.
    pub fn active_count(&self) -> Result<u32, HostError> {
        // SAFETY: структура из целых чисел, ноль корректен.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: указатель и длина описывают живую структуру `info`; handle валиден.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(HostError::Io(format!(
                "QueryInformationJobObject(учёт): {}",
                last_error()
            )));
        }
        Ok(info.ActiveProcesses)
    }

    /// Завершает все процессы job с этим кодом выхода.
    pub fn terminate(&self, code: u32) -> Result<(), HostError> {
        // SAFETY: handle job валиден.
        let ok = unsafe { TerminateJobObject(self.handle, code) };
        if ok == 0 {
            return Err(HostError::Io(format!(
                "TerminateJobObject: {}",
                last_error()
            )));
        }
        Ok(())
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: handle наш и закрывается ровно один раз; с KILL_ON_JOB_CLOSE это завершает оставшиеся процессы.
        unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Liveness;
    use crate::win::process::{OwnedProc, snapshot_processes};
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    /// Опрашивает условие до 2 с.
    fn within_2s(mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        cond()
    }

    fn spawn_ping() -> Child {
        Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .expect("cmd должен запуститься")
    }

    #[test]
    fn child_lands_in_job_and_terminate_kills_the_tree() {
        let job = Job::new().unwrap();
        let mut child = spawn_ping();
        job.assign_child(&child).unwrap();
        let me = OwnedProc::open(child.id()).unwrap();

        assert!(job.pids().unwrap().contains(&child.id()), "cmd в job");
        let ping_in_job = within_2s(|| {
            let names = snapshot_processes().unwrap();
            job.pids().unwrap().iter().any(|p| {
                names
                    .iter()
                    .any(|e| e.pid == *p && e.exe_name.eq_ignore_ascii_case("ping.exe"))
            })
        });
        assert!(
            ping_in_job,
            "потомок cmd (PING.EXE) тоже в job: {:?}",
            job.pids()
        );
        assert_eq!(me.liveness(), Liveness::Alive);
        assert!(job.active_count().unwrap() >= 2);

        job.terminate(1).unwrap();
        assert!(
            within_2s(|| me.liveness() == Liveness::Exited(Some(1))),
            "после terminate: {:?}",
            me.liveness()
        );
        assert!(
            within_2s(|| job.active_count().unwrap() == 0),
            "в job остались процессы: {:?}",
            job.pids()
        );
        let _ = child.wait();
    }

    #[test]
    fn dropping_the_job_kills_its_processes() {
        let job = Job::new().unwrap();
        let mut child = spawn_ping();
        job.assign_child(&child).unwrap();
        let me = OwnedProc::open(child.id()).unwrap();
        assert_eq!(me.liveness(), Liveness::Alive);
        drop(job);
        assert!(
            within_2s(|| matches!(me.liveness(), Liveness::Exited(_))),
            "после закрытия job процесс жив: {:?}",
            me.liveness()
        );
        let _ = child.wait();
    }
}
