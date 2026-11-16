use std::{
    env,
    ffi::{CStr, c_char, c_void},
    fs,
    path::{Path, PathBuf},
    ptr, thread,
    time::{Duration, Instant},
};

use libloading::Library;

type Handle = *mut c_void;
type Open = unsafe extern "system" fn(*mut Handle) -> i32;
type Load = unsafe extern "system" fn(Handle, *const u8, usize) -> i32;
type Execute = unsafe extern "system" fn(
    Handle,
    *const c_char,
    *const u64,
    usize,
    *mut u64,
    usize,
    *mut usize,
) -> i32;
type Close = unsafe extern "system" fn(Handle) -> i32;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn ReleaseMutex(handle: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

struct IsaBus(Handle);

impl IsaBus {
    fn lock() -> Result<Self, String> {
        let name: Vec<u16> = r"Global\Access_ISABUS.HTP.Method"
            .encode_utf16()
            .chain([0])
            .collect();
        // SAFETY: The name is NUL-terminated and no security attributes are supplied.
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(format!("ISA mutex: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: The handle came from CreateMutexW and remains owned here.
        let wait = unsafe { WaitForSingleObject(handle, 500) };
        if matches!(wait, 0 | 0x80) {
            Ok(Self(handle))
        } else {
            // SAFETY: No ownership of the mutex was acquired. Close only the handle.
            unsafe { CloseHandle(handle) };
            Err(format!(
                "could not acquire ISA bus mutex (wait status 0x{wait:X})"
            ))
        }
    }
}

impl Drop for IsaBus {
    fn drop(&mut self) {
        // SAFETY: This guard is used and dropped on the probing thread.
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

struct LpcIo {
    handle: Handle,
    execute: Execute,
    close: Close,
    _library: Library,
}

fn check(operation: &str, status: i32) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{operation}: HRESULT 0x{:08X}", status as u32))
    }
}

impl LpcIo {
    fn open(module: &Path) -> Result<Self, String> {
        let program_files = env::var_os("ProgramW6432")
            .or_else(|| env::var_os("ProgramFiles"))
            .ok_or("Program Files unavailable")?;
        let dll = PathBuf::from(program_files).join(r"PawnIO\PawnIOLib.dll");
        if !dll.is_file() {
            return Err(format!("PawnIO is not installed at {}", dll.display()));
        }
        let blob = fs::read(module).map_err(|e| format!("{}: {e}", module.display()))?;
        // SAFETY: Use the PawnIO installation's DLL and the documented signatures.
        // The library stays loaded for the lifetime of all resolved functions.
        unsafe {
            let library = Library::new(dll).map_err(|e| e.to_string())?;
            let open = *library
                .get::<Open>(b"pawnio_open")
                .map_err(|e| e.to_string())?;
            let load = *library
                .get::<Load>(b"pawnio_load")
                .map_err(|e| e.to_string())?;
            let execute = *library
                .get::<Execute>(b"pawnio_execute")
                .map_err(|e| e.to_string())?;
            let close = *library
                .get::<Close>(b"pawnio_close")
                .map_err(|e| e.to_string())?;
            let mut handle = ptr::null_mut();
            check(
                "pawnio_open (run from an administrator terminal)",
                open(&mut handle),
            )?;
            let io = Self {
                handle,
                execute,
                close,
                _library: library,
            };
            check("pawnio_load LpcIO", load(handle, blob.as_ptr(), blob.len()))?;
            Ok(io)
        }
    }

    fn call(&self, name: &CStr, args: &[u64], output: &mut [u64]) -> Result<(), String> {
        let mut count = 0;
        // SAFETY: The module session is alive and input/output buffers match the
        // lengths provided to the DLL. Empty buffers are passed as null.
        let status = unsafe {
            (self.execute)(
                self.handle,
                name.as_ptr(),
                if args.is_empty() {
                    ptr::null()
                } else {
                    args.as_ptr()
                },
                args.len(),
                if output.is_empty() {
                    ptr::null_mut()
                } else {
                    output.as_mut_ptr()
                },
                output.len(),
                &mut count,
            )
        };
        check(&name.to_string_lossy(), status)?;
        if count != output.len() {
            return Err(format!(
                "{}: expected {} output values, got {count}",
                name.to_string_lossy(),
                output.len()
            ));
        }
        Ok(())
    }

    fn read(&self, operation: &CStr, address: u16) -> Result<u64, String> {
        let mut output = [0];
        self.call(operation, &[address.into()], &mut output)?;
        Ok(output[0])
    }

    // Only used for protocol selectors. Never writes a fan setting or EC data.
    fn select_port(&self, port: u16, value: u8) -> Result<(), String> {
        self.call(c"ioctl_pio_outb", &[port.into(), value.into()], &mut [])
    }

    fn discover(&self, slot: u64, port: u16) -> Result<Option<u16>, String> {
        self.call(c"ioctl_select_slot", &[slot], &mut [])?;
        self.select_port(port, 0x87)?;
        let result = (|| {
            self.select_port(port, 0x87)?;
            if self.read(c"ioctl_superio_inw", 0x20)? != 0xD592 {
                return Ok(None);
            }
            let ldn = self.read(c"ioctl_superio_inb", 0x07)?;
            self.call(c"ioctl_find_bars", &[], &mut [])?;
            self.call(c"ioctl_superio_outb", &[0x07, 0x0B], &mut [])?;
            let base = self.read(c"ioctl_superio_inw", 0x60);
            let restore = self.call(c"ioctl_superio_outb", &[0x07, ldn], &mut []);
            let base = base?;
            restore?;
            if !(0x100..=0x0FF8).contains(&base) || base & 7 != 0 {
                return Err(format!("invalid hardware-monitor base 0x{base:X}"));
            }
            Ok(Some(base as u16))
        })();
        let exit = self.select_port(port, 0xAA);
        result.and_then(|value| exit.map(|()| value))
    }

    fn tach_byte(&self, base: u16, register: u16) -> Result<u8, String> {
        let page_port = base + 4;
        let deadline = Instant::now() + Duration::from_millis(100);
        while self.read(c"ioctl_pio_inb", page_port)? != 0xFF {
            if Instant::now() >= deadline {
                return Err("embedded controller did not release its page selector".into());
            }
            thread::sleep(Duration::from_millis(1));
        }
        let [page, index] = register.to_be_bytes();
        self.select_port(page_port, page)?;
        let result = self
            .select_port(base + 5, index)
            .and_then(|()| self.read(c"ioctl_pio_inb", base + 6));
        let release = self.select_port(page_port, 0xFF);
        result.and_then(|value| release.map(|()| value as u8))
    }

    fn rpm(&self, base: u16, register: u16) -> Result<Option<u16>, String> {
        // If the counter rolls over between byte reads, its high byte changes.
        for _ in 0..3 {
            let high = self.tach_byte(base, register)?;
            let low = self.tach_byte(base, register + 1)?;
            if self.tach_byte(base, register)? == high {
                let rpm = u16::from_be_bytes([high, low]);
                return Ok((!matches!(rpm, 0xFF00 | 0xFFF8 | 0xFFFF)).then_some(rpm));
            }
        }
        Err("tachometer value changed during sampling".into())
    }
}

impl Drop for LpcIo {
    fn drop(&mut self) {
        // SAFETY: The handle is owned here and the DLL is still loaded.
        unsafe { (self.close)(self.handle) };
    }
}

/// On the 6687 family, CPU and pump are the first tachometers. System fan
/// assignments differ between the classic and extended layouts.
/// Register reference: fancontrol-rs, fancontrol-pawnio/src/nct668.rs.
pub fn report(module: &Path) -> Result<(), String> {
    let io = LpcIo::open(module)?;
    let _bus = IsaBus::lock()?;
    for (slot, port) in [(0, 0x2E), (1, 0x4E)] {
        let Some(base) = io.discover(slot, port)? else {
            continue;
        };
        println!("  NCT6687 family (ID D592, monitor base 0x{base:04X})");
        println!("  System-fan labels are candidate mappings until verified against the board.");
        for (label, register) in [("CPU_FAN1", 0x140), ("PUMP_FAN1", 0x142)] {
            print_tach(label, io.rpm(base, register));
        }
        for (fan, (classic, extended)) in [(0x144, 0x15E), (0x146, 0x15C), (0x148, 0x15A)]
            .into_iter()
            .enumerate()
        {
            let label = format!("SYS_FAN{}", fan + 1);
            print_tach(
                &format!("{label} classic (0x{classic:03X})"),
                io.rpm(base, classic),
            );
            print_tach(
                &format!("{label} extended (0x{extended:03X})"),
                io.rpm(base, extended),
            );
        }
        return Ok(());
    }
    Err("Nuvoton NCT6687 family chip ID D592 was not found".into())
}

fn print_tach(label: &str, rpm: Result<Option<u16>, String>) {
    match rpm {
        Ok(Some(rpm)) => println!("  {label}: {rpm} RPM"),
        Ok(None) => println!("  {label}: unavailable (no valid tachometer data)"),
        Err(error) => println!("  {label}: unavailable ({error})"),
    }
}
