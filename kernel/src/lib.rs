#![no_std]

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[link(wasm_import_module = "host")]
extern "C" {
    fn console_write(ptr: *const u8, len: usize);
}

fn print(s: &str) {
    unsafe { console_write(s.as_ptr(), s.len()) }
}

#[no_mangle]
pub extern "C" fn kmain() {
    print("MavexOS kernel booting...\n");
    print("kernel: wasm32 core online\n");
    print("mavex$ ");
}
