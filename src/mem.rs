use std::ffi::c_void;
use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
    PROCESS_VM_WRITE,
};

const PROCESS_ACCESS: u32 =
    PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION;

pub struct Process {
    handle: HANDLE,
}

unsafe impl Send for Process {}

impl Drop for Process {
    fn drop(&mut self) {
        if !self.handle.is_null() && self.handle != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

impl Process {
    pub fn open_by_name(name: &str) -> Result<Self, String> {
        let pid = find_pid(name).ok_or_else(|| format!("process not found: {name}"))?;
        Self::open_pid(pid)
    }

    pub fn open_pid(pid: u32) -> Result<Self, String> {
        let handle = unsafe { OpenProcess(PROCESS_ACCESS, FALSE, pid) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "OpenProcess({pid}) failed — run as Administrator?"
            ));
        }
        Ok(Self { handle })
    }

    pub fn read_bytes(&self, addr: u64, buf: &mut [u8]) -> bool {
        if addr == 0 || buf.is_empty() {
            return false;
        }
        let mut n = 0usize;
        let ok = unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory(
                self.handle,
                addr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                &mut n,
            )
        };
        ok != 0 && n == buf.len()
    }

    pub fn write_bytes(&self, addr: u64, buf: &[u8]) -> bool {
        if addr == 0 || buf.is_empty() {
            return false;
        }
        let mut n = 0usize;
        let ok = unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory(
                self.handle,
                addr as *mut c_void,
                buf.as_ptr() as *const c_void,
                buf.len(),
                &mut n,
            )
        };
        ok != 0 && n == buf.len()
    }

    pub fn read_u64(&self, addr: u64) -> Option<u64> {
        let mut b = [0u8; 8];
        if self.read_bytes(addr, &mut b) {
            Some(u64::from_le_bytes(b))
        } else {
            None
        }
    }

    pub fn read_f32(&self, addr: u64) -> Option<f32> {
        let mut b = [0u8; 4];
        if self.read_bytes(addr, &mut b) {
            Some(f32::from_le_bytes(b))
        } else {
            None
        }
    }

    pub fn write_f32(&self, addr: u64, v: f32) -> bool {
        self.write_bytes(addr, &v.to_le_bytes())
    }

    pub fn write_u8(&self, addr: u64, v: u8) -> bool {
        self.write_bytes(addr, &[v])
    }

    pub fn readable_regions(&self) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut addr: u64 = 0x10000;
        let max: u64 = 0x0000_7FFF_FFFF_FFFF;
        while addr < max {
            let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
            let n = unsafe {
                VirtualQueryEx(
                    self.handle,
                    addr as *const c_void,
                    &mut mbi,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if n == 0 {
                break;
            }
            let base = mbi.BaseAddress as u64;
            let size = mbi.RegionSize as u64;
            let prot = mbi.Protect;
            let state = mbi.State;
            let readable = matches!(
                prot,
                PAGE_READONLY
                    | PAGE_READWRITE
                    | PAGE_WRITECOPY
                    | PAGE_EXECUTE_READ
                    | PAGE_EXECUTE_READWRITE
            );
            if state == MEM_COMMIT && readable && size > 0 {
                out.push((base, size));
            }
            let next = base.saturating_add(size);
            if next <= addr {
                break;
            }
            addr = next;
        }
        out
    }

    pub fn looks_like_user_ptr(&self, p: u64) -> bool {
        if p < 0x10000 || p > 0x0000_7FFF_FFFF_FFFF || (p & 7) != 0 {
            return false;
        }
        let mut b = [0u8; 8];
        self.read_bytes(p, &mut b)
    }
}

fn find_pid(name: &str) -> Option<u32> {
    let name_l = name.to_ascii_lowercase();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut pe: PROCESSENTRY32W = unsafe { zeroed() };
    pe.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut found = None;
    unsafe {
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                let exe = widestr(&pe.szExeFile);
                if exe.to_ascii_lowercase() == name_l {
                    found = Some(pe.th32ProcessID);
                    break;
                }
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    found
}

fn widestr(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}
