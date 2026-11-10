//! Memory measurements for development, printed to the emulator's log by `--features debug`
//! builds. The stack and the heap share 36 KB, so both are worth knowing exactly.

#[cfg(feature = "debug")]
mod measure {
    use alloc::{format, vec::Vec};
    use core::arch::asm;

    unsafe extern "C" {
        static _stack: u8;
        static _estack: u8;
    }

    const PAINT: u8 = 0xa5;

    fn stack_pointer() -> usize {
        let pointer: usize;
        unsafe { asm!("mov {}, sp", out(reg) pointer) };
        pointer
    }

    fn bounds() -> (usize, usize) {
        (&raw const _stack as usize, &raw const _estack as usize)
    }

    /// Fill the unused stack with a pattern, so later reports can see how deep it went.
    pub fn paint() {
        let (bottom, _) = bounds();
        let top = stack_pointer() - 128;
        for address in bottom..top {
            unsafe { core::ptr::write_volatile(address as *mut u8, PAINT) };
        }
    }

    /// The largest block the heap can still give.
    fn largest_free_block() -> usize {
        let (mut low, mut high) = (0_usize, 40_000_usize);
        while high - low > 64 {
            let middle = (low + high) / 2;
            let mut probe = Vec::<u8>::new();
            if probe.try_reserve_exact(middle).is_ok() {
                low = middle;
            } else {
                high = middle;
            }
        }
        low
    }

    pub fn report(label: &str) {
        let (bottom, top) = bounds();
        let mut deepest = bottom;
        while deepest < top && unsafe { core::ptr::read_volatile(deepest as *const u8) } == PAINT {
            deepest += 1;
        }
        #[allow(deprecated)]
        ledger_device_sdk::testing::debug_print(&format!(
            "memory {label}: stack {} of {} used at its deepest, {} now; heap's largest free block {}\n",
            top - deepest,
            top - bottom,
            top - stack_pointer(),
            largest_free_block(),
        ));
    }
}

#[cfg(feature = "debug")]
pub use measure::{paint, report};

/// Print a panic's location. No allocation: the heap may be what failed.
#[cfg(feature = "debug")]
pub fn note(info: &core::panic::PanicInfo) {
    #[allow(deprecated)]
    use ledger_device_sdk::testing::debug_print;
    debug_print("panic at ");
    if let Some(location) = info.location() {
        debug_print(location.file());
        let mut digits = [0_u8; 10];
        let (mut line, mut start) = (location.line(), digits.len());
        while line > 0 {
            start -= 1;
            digits[start] = b'0' + (line % 10) as u8;
            line /= 10;
        }
        debug_print(":");
        debug_print(core::str::from_utf8(&digits[start..]).unwrap_or("?"));
    }
    debug_print("\n");
}
#[cfg(not(feature = "debug"))]
pub fn note(_info: &core::panic::PanicInfo) {}

#[cfg(not(feature = "debug"))]
pub fn paint() {}
#[cfg(not(feature = "debug"))]
pub fn report(_label: &str) {}
